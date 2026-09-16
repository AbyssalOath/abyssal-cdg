//! Lyric data model.
//!
//! A song is a list of [`LyricLine`]s. Each line has a start time (seconds)
//! set by the user (typically via "tap to time" while the song plays). The
//! end time of a line is implicitly the start time of the next line (or the
//! end of the audio, for the last line).
//!
//! Word-level highlight timing (for the classic karaoke color-wipe effect)
//! is *not* stored explicitly - it's derived from the line's estimated
//! singing duration and each word's character length, so the user only ever
//! has to tap once per line instead of once per word.
//!
//! A line's *display* window (start..end) can be much longer than the time
//! actually spent singing it (e.g. there's a long instrumental break before
//! the next line). We estimate how long the words actually take to sing,
//! and treat any big leftover gap as a "break" - which is what
//! [`countdown_window`] uses to trigger the get-ready countdown indicator.

/// Which voice sings a line - lets duet songs color each singer's lines
/// differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Singer {
    #[default]
    Male,
    Female,
    Duet,
    /// A 4th voice category for an intense/screamed part - distinct from
    /// Male/Female/Duet so a song with a screaming section (metal, rap
    /// ad-libs, etc.) can color it differently from the rest.
    Screaming,
}

impl Singer {
    pub fn label(self) -> &'static str {
        match self {
            Self::Male => "Male",
            Self::Female => "Female",
            Self::Duet => "Duet",
            Self::Screaming => "Screaming",
        }
    }
}

/// Rough estimate of how long it takes to sing `text`, used only to decide
/// how much of a line's on-screen window is "still being sung" vs. "already
/// finished, waiting for the next line" (see [`countdown_window`]). This is
/// a heuristic, not a measurement - if it's off, the worst case is the
/// color wipe finishes a bit early or late within the line, not anything
/// broken.
pub const SECONDS_PER_WORD: f64 = 0.45;
pub const MIN_SING_DURATION: f64 = 1.2;

/// A gap after a line's estimated singing ends must be at least this long
/// before we bother showing a countdown indicator (short natural pauses
/// between lines shouldn't trigger it).
pub const COUNTDOWN_GAP_THRESHOLD: f64 = 5.0;
/// The countdown indicator occupies this many seconds immediately before
/// the next line begins.
pub const COUNTDOWN_LEAD_SECS: f64 = 4.0;

/// How long an already-sung line (and any earlier lines still shown
/// highlighted in the same block) lingers on screen after singing ends,
/// during a break long enough to trigger the countdown indicator. Past
/// this point the display goes blank (until the countdown dots appear)
/// instead of leaving finished lyrics sitting there for the whole break.
pub const SUNG_LINGER_SECS: f64 = 5.0;

fn estimate_sing_duration(text: &str, window: f64) -> f64 {
    let word_count = text.split_whitespace().count().max(1);
    let est = (word_count as f64 * SECONDS_PER_WORD).max(MIN_SING_DURATION);
    est.min(window.max(0.0))
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LyricLine {
    pub text: String,
    /// Start time in seconds. `None` means "not yet timed".
    pub start: Option<f64>,
    pub singer: Singer,
    /// One slot per word (same order as `text.split_whitespace()`). `Some(t)`
    /// means the user manually tapped/adjusted that word's highlight time;
    /// `None` means "use the automatic estimate" (the default for every
    /// word until the user fine-tunes it). Mixing manual and automatic
    /// words within the same line is fine.
    pub word_overrides: Vec<Option<f64>>,
    /// One slot per word - manual override for when a word's held-out
    /// highlight should *end* (i.e. when the color-wipe should stop
    /// advancing through its letters and freeze, waiting for the next
    /// word). `None` means "use the automatic default": the next word's
    /// (manual-or-estimated) start, or [`LyricLine::sing_end_override`]/the
    /// automatic estimate for the line's last word - i.e. a continuous
    /// wipe with no pause, which is what every word gets until fine-tuned.
    /// Setting this lets a word's highlight stop *before* the next word's
    /// start (e.g. a held note followed by a musical pause before the next
    /// word), instead of always stretching to fill that whole gap.
    pub word_end_overrides: Vec<Option<f64>>,
    /// Manual override for when this line's singing actually finishes (i.e.
    /// when the last word should stop being held out and the color-wipe
    /// should be complete). `None` means "use the automatic estimate" (see
    /// [`estimate_sing_duration`]), which is only ever a heuristic based on
    /// word count - fine for lines the user hasn't fine-tuned, but often not
    /// accurate enough once every word's *start* has been manually tapped,
    /// since the estimate has no way to know when the last word was actually
    /// finished being sung. Set by tapping "End of line" in the fine-tune
    /// panel while the last word is still playing. Also used by
    /// [`countdown_window`] to know when this line's musical break (if any)
    /// begins, so it's worth setting even if the last word already has its
    /// own [`word_end_overrides`] entry.
    pub sing_end_override: Option<f64>,
    /// True if this line should start a new on-screen verse block in the
    /// video export - i.e. it was preceded by a blank line in the pasted
    /// lyrics (or it's the very first line). Defaults to `true` for lines
    /// created outside of [`parse_pasted_lyrics`] (e.g. in tests), since
    /// treating every line as its own block is a safer default than
    /// silently merging unrelated lines together.
    pub starts_new_block: bool,
}

impl LyricLine {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let word_count = text.split_whitespace().count();
        Self {
            text,
            start: None,
            singer: Singer::default(),
            word_overrides: vec![None; word_count],
            word_end_overrides: vec![None; word_count],
            sing_end_override: None,
            starts_new_block: true,
        }
    }

    pub fn words(&self) -> Vec<&str> {
        self.text.split_whitespace().collect()
    }
}

/// True for a line that's *entirely* wrapped in square brackets, like
/// `[Verse 1]`, `[Chorus: Some Artist]`, or `[Instrumental Break]` - the
/// convention lyrics sites (Genius and others) use for structural section
/// annotations. These are never meant to be sung or displayed, so
/// [`parse_pasted_lyrics`] drops them rather than turning each one into an
/// extra "line" the user would otherwise have to tap a timestamp for.
/// Deliberately narrow: a line with brackets *somewhere* in it (background
/// vocals, ad-libs) is left alone - only a line that's nothing *but* a
/// bracketed annotation counts.
fn is_section_marker(line: &str) -> bool {
    line.len() >= 2 && line.starts_with('[') && line.ends_with(']')
}

/// Parse raw pasted lyrics text into lines, one [`LyricLine`] per non-empty,
/// non-section-marker input line (see [`is_section_marker`]). Blank lines
/// and section markers are both dropped (they'd otherwise become an empty
/// or untimeable-in-spirit line) but their *position* is preserved: a line
/// immediately following one or more of them is marked as starting a new
/// verse block (see [`LyricLine::starts_new_block`]), which the video
/// export uses to group lines into on-screen blocks - this is why leaving
/// blank lines (or a `[Chorus]`-style header) between verses/stanzas in
/// your pasted lyrics is worth doing, even though neither becomes a
/// timeable entry itself.
pub fn parse_pasted_lyrics(raw: &str) -> Vec<LyricLine> {
    let mut result = Vec::new();
    let mut pending_blank = false;
    for raw_line in raw.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || is_section_marker(trimmed) {
            pending_blank = true;
            continue;
        }
        let mut line = LyricLine::new(trimmed);
        line.starts_new_block = result.is_empty() || pending_blank;
        result.push(line);
        pending_blank = false;
    }
    result
}

/// Formats seconds as `MM:SS.CC` (centiseconds) for the editable start/end
/// time fields - more precision than [`crate::format_time`]'s coarser
/// deciseconds display used elsewhere, since these are meant to be typed
/// back in. Negative/non-finite input formats as `00:00.00` rather than
/// panicking or producing a nonsense string, since a partially-edited field
/// can transiently hold one.
pub fn format_timecode(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "00:00.00".to_string();
    }
    let total_cs = (secs * 100.0).round() as i64;
    let m = total_cs / 6000;
    let s = (total_cs / 100) % 60;
    let c = total_cs % 100;
    format!("{m:02}:{s:02}.{c:02}")
}

/// Parses a `[MM:]SS[.frac]` timestamp (as produced by [`format_timecode`],
/// but tolerant of a missing minutes part, missing/extra fractional
/// digits, and surrounding whitespace) back into seconds. Returns `None`
/// for anything that doesn't look like a time (rather than a default of
/// `0.0`, so callers can tell "invalid" apart from "typed zero").
pub fn parse_timecode(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (mins, secs_part) = match s.rsplit_once(':') {
        Some((m, rest)) => (m.trim().parse::<f64>().ok()?, rest),
        None => (0.0, s),
    };
    let secs = secs_part.trim().parse::<f64>().ok()?;
    if !mins.is_finite() || !secs.is_finite() || mins < 0.0 || secs < 0.0 {
        return None;
    }
    Some(mins * 60.0 + secs)
}

/// Checks whether setting `lines[idx]`'s `start` to `new_start` is valid -
/// non-negative, before this line's own explicit end (if any), and not
/// before the previous line's explicit end (if any) - *without* applying
/// it. Callers (tapping, dragging, or typing a new start) should only
/// commit the change once this returns `Ok`, and show the message to the
/// user otherwise.
///
/// Deliberately only checks against an explicit [`LyricLine::sing_end_override`]
/// on either side, not the automatic estimate - the estimate shifts as
/// timing fills in around it, so treating it as a hard wall would reject
/// perfectly reasonable taps just because a neighboring line hasn't been
/// fine-tuned yet.
pub fn check_start_change(lines: &[LyricLine], idx: usize, new_start: f64) -> Result<(), String> {
    if !new_start.is_finite() || new_start < 0.0 {
        return Err("Start time can't be negative.".to_string());
    }
    if let Some(end) = lines[idx].sing_end_override {
        if new_start >= end {
            return Err(format!(
                "Start ({}) must be before this line's own end ({}).",
                format_timecode(new_start),
                format_timecode(end)
            ));
        }
    }
    if idx > 0 {
        if let Some(prev_end) = lines[idx - 1].sing_end_override {
            if new_start < prev_end {
                return Err(format!(
                    "Start ({}) can't be before the previous line ends ({}).",
                    format_timecode(new_start),
                    format_timecode(prev_end)
                ));
            }
        }
    }
    Ok(())
}

/// Checks whether setting `lines[idx]`'s [`LyricLine::sing_end_override`] to
/// `new_end` is valid - after this line's own start, and not after the
/// next line's start (if that's already been set) - *without* applying it.
/// See [`check_start_change`] for why only explicit neighboring times are
/// checked, not automatic estimates.
pub fn check_end_change(lines: &[LyricLine], idx: usize, new_end: f64) -> Result<(), String> {
    let Some(start) = lines[idx].start else {
        return Err("Set this line's start before its end.".to_string());
    };
    if !new_end.is_finite() || new_end <= start {
        return Err(format!(
            "End ({}) must be after this line's start ({}).",
            format_timecode(new_end),
            format_timecode(start)
        ));
    }
    if let Some(next) = lines.get(idx + 1) {
        if let Some(next_start) = next.start {
            if new_end > next_start {
                return Err(format!(
                    "End ({}) can't be after the next line starts ({}).",
                    format_timecode(new_end),
                    format_timecode(next_start)
                ));
            }
        }
    }
    Ok(())
}

/// A line with its resolved [start, end) window, used at export/preview time.
#[derive(Clone, Debug)]
pub struct TimedLine {
    pub text: String,
    pub start: f64,
    /// When the *next* line begins (or the end of the song, for the last line).
    pub end: f64,
    /// Estimated end of active singing for *this* line; always `<= end`.
    /// The word color-wipe is spread across `[start, sing_end)`. Any gap
    /// between `sing_end` and `end` is a musical break.
    pub sing_end: f64,
    pub singer: Singer,
    /// See [`LyricLine::word_overrides`].
    pub word_overrides: Vec<Option<f64>>,
    /// See [`LyricLine::word_end_overrides`].
    pub word_end_overrides: Vec<Option<f64>>,
    /// See [`LyricLine::starts_new_block`].
    pub starts_new_block: bool,
}

impl TimedLine {
    #[allow(dead_code)]
    pub fn new(text: String, start: f64, end: f64, singer: Singer) -> Self {
        let mut line = LyricLine::new(text);
        line.singer = singer;
        Self::from_line(&line, start, end)
    }

    /// Build a resolved [start, end) window for `line`, carrying over its
    /// word/end/singer overrides. Used by [`resolve_timing`] and by the live
    /// preview (which needs the same resolution but also wants to know
    /// which original `LyricLine` each result came from).
    pub fn from_line(line: &LyricLine, start: f64, end: f64) -> Self {
        let window = (end - start).max(0.0);
        let sing_end = match line.sing_end_override {
            Some(manual) => manual.clamp(start, end),
            None => (start + estimate_sing_duration(&line.text, window)).clamp(start, end),
        };
        Self {
            text: line.text.clone(),
            start,
            end,
            sing_end,
            singer: line.singer,
            word_overrides: line.word_overrides.clone(),
            word_end_overrides: line.word_end_overrides.clone(),
            starts_new_block: line.starts_new_block,
        }
    }
}

/// Resolve start/end windows for every line. Requires every line to already
/// have a `start` set (caller should validate this first). Lines are sorted
/// by start time. The final line's end is `total_duration` (or `start + 4.0`
/// if `total_duration` is unknown / shorter than that).
pub fn resolve_timing(lines: &[LyricLine], total_duration: Option<f64>) -> Vec<TimedLine> {
    let mut sorted: Vec<(f64, &LyricLine)> = lines
        .iter()
        .filter_map(|l| l.start.map(|s| (s, l)))
        .collect();
    sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    let mut out = Vec::with_capacity(sorted.len());
    for i in 0..sorted.len() {
        let (start, line) = sorted[i];
        let end = if i + 1 < sorted.len() {
            sorted[i + 1].0
        } else {
            match total_duration {
                Some(d) if d > start => d,
                _ => start + 4.0,
            }
        };
        out.push(TimedLine::from_line(line, start, end));
    }
    out
}

/// A word within a line, with its derived highlight start/end times
/// (proportional to its position within the line's *estimated singing
/// window*, weighted by character count) - or the user's manually tapped
/// times, if set.
pub struct TimedWord<'a> {
    #[allow(dead_code)]
    pub text: &'a str,
    pub highlight_at: f64,
    /// When this word's held-out highlight should stop advancing (and
    /// freeze, fully colored, until the next word begins). Defaults to the
    /// next word's `highlight_at` (a continuous wipe with no pause) or, for
    /// the line's last word, `line.sing_end` - unless manually overridden
    /// (see [`LyricLine::word_end_overrides`]), in which case a gap can open
    /// up between this word finishing and the next one starting.
    pub held_until: f64,
    #[allow(dead_code)]
    pub is_manual: bool,
    #[allow(dead_code)]
    pub end_is_manual: bool,
}

/// Split a timed line into words, each with a derived highlight start/end
/// spread across `[line.start, line.sing_end)`, proportional to cumulative
/// character count - except where the user has manually overridden a
/// specific word's start and/or end time, which takes precedence.
pub fn word_timings(line: &TimedLine) -> Vec<TimedWord<'_>> {
    let words: Vec<&str> = line.text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let total_chars: usize = words
        .iter()
        .map(|w| w.chars().count())
        .sum::<usize>()
        .max(1);
    let duration = (line.sing_end - line.start).max(0.05);

    // Resolve every word's *start* first (manual override, or the automatic
    // character-weighted estimate), so each word's automatic *end* can then
    // be defined as "the next word's resolved start" - matching the
    // historical continuous-wipe behavior whenever no end is manually set.
    let mut resolved_starts = Vec::with_capacity(words.len());
    let mut chars_so_far = 0usize;
    for (i, w) in words.iter().enumerate() {
        let frac = chars_so_far as f64 / total_chars as f64;
        let auto_start = line.start + duration * frac;
        let manual_start = line.word_overrides.get(i).copied().flatten();
        resolved_starts.push(manual_start.unwrap_or(auto_start));
        chars_so_far += w.chars().count();
    }

    let mut out = Vec::with_capacity(words.len());
    for (i, w) in words.iter().enumerate() {
        let start = resolved_starts[i];
        let manual_start = line.word_overrides.get(i).copied().flatten();
        let default_end = resolved_starts.get(i + 1).copied().unwrap_or(line.sing_end);
        let manual_end = line.word_end_overrides.get(i).copied().flatten();
        out.push(TimedWord {
            text: w,
            highlight_at: start,
            held_until: manual_end.unwrap_or(default_end).max(start),
            is_manual: manual_start.is_some(),
            end_is_manual: manual_end.is_some(),
        });
    }
    out
}

/// Re-join words with a single space - this is what's actually measured
/// and drawn, and what word/char spans are computed against, so they
/// always agree regardless of whatever whitespace was in the source text.
pub fn normalize_text(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// (char offset within the normalized text, char length) for each word.
pub fn word_char_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut offset = 0usize;
    for w in text.split_whitespace() {
        let len = w.chars().count();
        spans.push((offset, len));
        offset += len + 1;
    }
    spans
}

/// A continuous (not stepped) 0.0..=1.0 fraction of the way across the
/// currently-singing line, at time `t`. This is what drives the karaoke
/// color wipe in both the video exporter and the live preview. It's still
/// *anchored* to each word's own timestamps (from [`word_timings`]) for
/// accuracy, reaching the right fraction at the right moment for each word,
/// but linearly interpolates within `[highlight_at, held_until)` instead of
/// jumping, so the wipe moves continuously through a word's letters. Once a
/// word reaches its own `held_until`, the fraction freezes there (fully
/// colored) until the next word's `highlight_at` is reached - which is
/// instantaneous (no freeze) for words that don't have a manual end
/// override, since their `held_until` already *is* the next word's start.
pub fn current_line_wipe_fraction(line: &TimedLine, t: f64) -> f32 {
    let text = normalize_text(&line.text);
    let char_count = text.chars().count().max(1) as f32;
    let spans = word_char_spans(&text);
    let words = word_timings(line);
    if words.is_empty() || spans.is_empty() {
        return 0.0;
    }
    if t <= words[0].highlight_at {
        return 0.0;
    }

    for (i, &(offset, len)) in spans.iter().enumerate() {
        let Some(word) = words.get(i) else { continue };
        let word_start = word.highlight_at;
        let word_end = word.held_until.max(word_start + 0.05);
        let next_start = words.get(i + 1).map(|w| w.highlight_at);
        let frac_start = offset as f32 / char_count;
        let frac_end = (offset + len) as f32 / char_count;
        // Stay on this word for as long as `t` hasn't reached the next
        // word's start yet (holding at `frac_end` once past this word's own
        // end) - or, for the last word, for the rest of the line.
        let still_on_this_word = match next_start {
            Some(next) => t < next,
            None => true,
        };
        if still_on_this_word {
            let dur = (word_end - word_start).max(0.05);
            let local = ((t - word_start) / dur).clamp(0.0, 1.0) as f32;
            return frac_start + (frac_end - frac_start) * local;
        }
    }
    1.0
}

/// A verse block never shows more than this many lines at once - beyond
/// this it'd get cramped, so a long uninterrupted run of lines is split
/// into consecutive blocks instead.
pub const MAX_BLOCK_LINES: usize = 5;

/// Groups consecutive line indices into display blocks of at most
/// [`MAX_BLOCK_LINES`], splitting wherever [`TimedLine::starts_new_block`]
/// is set - i.e. wherever there was a blank line in the pasted lyrics - so
/// a block matches exactly what looked like one verse/stanza when you
/// pasted the lyrics in, regardless of how the actual singing timing
/// happens to fall (that's a separate concern - see the countdown
/// indicator, which is still timing-based). Shared by the video exporter
/// and the live preview so both group lines identically.
pub fn group_into_blocks(timed_lines: &[TimedLine]) -> Vec<Vec<usize>> {
    let mut blocks = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for (i, line) in timed_lines.iter().enumerate() {
        let starts_new = i == 0 || line.starts_new_block;
        if starts_new && !current.is_empty() {
            blocks.push(std::mem::take(&mut current));
        }
        current.push(i);
        if current.len() >= MAX_BLOCK_LINES {
            blocks.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    blocks
}

/// If there's a long enough gap between `after` (when singing/display
/// activity last stopped) and `next_start` (when the next thing begins),
/// returns the `(countdown_start, countdown_end)` window (in seconds) during
/// which a "get ready" countdown indicator should be shown. `countdown_end`
/// is always equal to `next_start`.
///
/// This is the general form shared by two call sites: the gap between one
/// line's estimated singing end and the next line's start ([`countdown_window`]),
/// and the gap between the title card fading (or song start, if there's no
/// title card) and the first lyric line's start - used by the CDG exporter,
/// the video exporter, and the live preview, so all three agree on when the
/// intro countdown appears.
pub fn countdown_window_between(after: f64, next_start: f64) -> Option<(f64, f64)> {
    let leftover = next_start - after;
    if leftover >= COUNTDOWN_GAP_THRESHOLD {
        let cd_start = (next_start - COUNTDOWN_LEAD_SECS).max(after);
        Some((cd_start, next_start))
    } else {
        None
    }
}

/// If a line has a long enough musical break before the *next* line starts,
/// returns the `(countdown_start, countdown_end)` window (in seconds) during
/// which a "get ready" countdown indicator should be shown. `countdown_end`
/// is always equal to `line.end` (i.e. the next line's start).
pub fn countdown_window(line: &TimedLine) -> Option<(f64, f64)> {
    countdown_window_between(line.sing_end, line.end)
}

/// True once `line` is done being sung (`t >= line.sing_end`) and there's a
/// real musical break before the next line (i.e. [`countdown_window`] would
/// fire). Any not-yet-started lines still visible in the same on-screen
/// block should be hidden while this is true, instead of sitting on screen
/// for the whole break - the screen should read as "done, waiting" (then
/// the countdown indicator, then the next line), not show lyrics that are
/// still a break away. Shared by the video exporter, the CDG exporter, and
/// the live preview so all three agree on when to hide ahead-of-time lines.
pub fn hide_upcoming_lines(line: &TimedLine, t: f64) -> bool {
    t >= line.sing_end && countdown_window(line).is_some()
}

/// True once the already-sung display for `line` (this line, plus any
/// earlier lines still shown highlighted in the same on-screen block)
/// should be cleared too, during a break long enough to trigger the
/// countdown indicator - either because it's lingered on screen for
/// [`SUNG_LINGER_SECS`] since singing finished, or because the countdown is
/// about to start (whichever comes first, so a short-but-still-qualifying
/// break doesn't wait out the full linger before the dots appear). Implies
/// [`hide_upcoming_lines`] is also true. Shared by the video exporter, the
/// CDG exporter, and the live preview.
pub fn blank_sung_lines(line: &TimedLine, t: f64) -> bool {
    match countdown_window(line) {
        Some((cd_start, _)) => t >= (line.sing_end + SUNG_LINGER_SECS).min(cd_start),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_drops_blank_lines() {
        let raw = "Hello world\n\nSecond line\n   \nThird";
        let lines = parse_pasted_lyrics(raw);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "Hello world");
        assert_eq!(lines[2].text, "Third");
        assert_eq!(lines[0].singer, Singer::Male); // default
    }

    #[test]
    fn blank_lines_mark_the_following_line_as_a_new_block() {
        let raw = "a\nb\n\nc\nd\ne\n\n\nf\ng";
        let lines = parse_pasted_lyrics(raw);
        let flags: Vec<bool> = lines.iter().map(|l| l.starts_new_block).collect();
        // a(first, block start), b(no gap, same block), c(after blank, new
        // block), d(same block), e(same block), f(after 2 blanks, still
        // just starts a new block), g(same block).
        assert_eq!(flags, vec![true, false, true, false, false, true, false]);
    }

    #[test]
    fn drops_genius_style_section_markers() {
        let raw = "[Verse 1]\nFirst line\nSecond line\n[Chorus: Some Artist]\nThird line\n\
                   [Instrumental Break]\n[Outro]\nFourth line";
        let lines = parse_pasted_lyrics(raw);
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["First line", "Second line", "Third line", "Fourth line"]
        );
    }

    #[test]
    fn a_section_marker_starts_a_new_block_like_a_blank_line_would() {
        let raw = "First line\n[Chorus]\nSecond line";
        let lines = parse_pasted_lyrics(raw);
        assert!(lines[0].starts_new_block);
        assert!(lines[1].starts_new_block);
    }

    #[test]
    fn only_a_line_that_is_entirely_bracketed_counts_as_a_marker() {
        // Ad-libs/background vocals in brackets mid-lyric are real content,
        // not a structural annotation, so they must not be dropped.
        let raw = "Hey [background vocal] yeah\n[Verse 1]\nReal line";
        let lines = parse_pasted_lyrics(raw);
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["Hey [background vocal] yeah", "Real line"]);
    }

    #[test]
    fn resolve_timing_chains_end_to_next_start() {
        let mut lines = vec![
            LyricLine::new("a"),
            LyricLine::new("b"),
            LyricLine::new("c"),
        ];
        lines[0].start = Some(1.0);
        lines[1].start = Some(3.0);
        lines[2].start = Some(6.0);
        let timed = resolve_timing(&lines, Some(10.0));
        assert_eq!(timed.len(), 3);
        assert_eq!(timed[0].start, 1.0);
        assert_eq!(timed[0].end, 3.0);
        assert_eq!(timed[1].end, 6.0);
        assert_eq!(timed[2].end, 10.0);
    }

    #[test]
    fn resolve_timing_skips_untimed_lines() {
        let mut lines = vec![
            LyricLine::new("a"),
            LyricLine::new("skip me"),
            LyricLine::new("c"),
        ];
        lines[0].start = Some(1.0);
        lines[2].start = Some(5.0);
        let timed = resolve_timing(&lines, Some(10.0));
        assert_eq!(timed.len(), 2);
        assert_eq!(timed[1].text, "c");
    }

    #[test]
    fn resolve_timing_carries_singer() {
        let mut lines = vec![LyricLine::new("a"), LyricLine::new("b")];
        lines[0].start = Some(1.0);
        lines[0].singer = Singer::Female;
        lines[1].start = Some(3.0);
        lines[1].singer = Singer::Duet;
        let timed = resolve_timing(&lines, Some(6.0));
        assert_eq!(timed[0].singer, Singer::Female);
        assert_eq!(timed[1].singer, Singer::Duet);
    }

    #[test]
    fn word_timings_are_monotonic_and_within_sing_window() {
        // Window is huge (10s) but only 4 short words, so sing_end should
        // land well before end - word timings should stay within that
        // smaller window, not stretch across the whole 10 seconds.
        let line = TimedLine::new("one two three four".into(), 10.0, 20.0, Singer::Male);
        assert!(line.sing_end < line.end);
        let words = word_timings(&line);
        assert_eq!(words.len(), 4);
        assert_eq!(words[0].highlight_at, 10.0);
        for pair in words.windows(2) {
            assert!(pair[1].highlight_at >= pair[0].highlight_at);
        }
        for w in &words {
            assert!(w.highlight_at >= 10.0 && w.highlight_at < line.sing_end + 1e-9);
        }
    }

    #[test]
    fn short_window_caps_sing_end_at_line_end() {
        // Lots of words, tiny window - sing_end must not exceed end.
        let text = "one two three four five six seven eight nine ten eleven twelve";
        let line = TimedLine::new(text.into(), 5.0, 6.0, Singer::Male);
        assert_eq!(line.sing_end, 6.0);
    }

    #[test]
    fn countdown_triggers_on_long_gap_only() {
        // Short line, huge window -> big gap -> countdown should trigger.
        let long_gap = TimedLine::new("hi".into(), 0.0, 20.0, Singer::Male);
        let cd = countdown_window(&long_gap);
        assert!(cd.is_some());
        let (start, end) = cd.unwrap();
        assert_eq!(end, 20.0);
        assert!(start < end);
        assert!(start >= long_gap.sing_end);

        // Tight line, no meaningful gap -> no countdown.
        let tight = TimedLine::new("hi there".into(), 0.0, 2.0, Singer::Male);
        assert!(countdown_window(&tight).is_none());
    }

    #[test]
    fn manual_word_override_takes_precedence_over_estimate() {
        let mut lines = vec![LyricLine::new("one two three four")];
        lines[0].start = Some(10.0);
        // Manually override the 3rd word ("three", index 2) to a specific time.
        lines[0].word_overrides[2] = Some(13.7);
        let timed = resolve_timing(&lines, Some(20.0));
        let words = word_timings(&timed[0]);
        assert_eq!(words.len(), 4);
        assert_eq!(words[2].highlight_at, 13.7);
        assert!(words[2].is_manual);
        assert!(!words[0].is_manual);
        assert!(!words[1].is_manual);
        assert!(!words[3].is_manual);
        // Untouched words still use the automatic estimate.
        assert_eq!(words[0].highlight_at, timed[0].start);
    }

    #[test]
    fn new_lines_have_no_manual_overrides_by_default() {
        let line = LyricLine::new("a b c");
        assert_eq!(line.word_overrides, vec![None, None, None]);
    }

    #[test]
    fn sing_end_override_takes_precedence_over_estimate() {
        let mut lines = vec![LyricLine::new("one two three four")];
        lines[0].start = Some(10.0);
        // The automatic estimate for 4 short words would land well before
        // the 20s window closes - a manual override should win instead.
        lines[0].sing_end_override = Some(17.5);
        let timed = resolve_timing(&lines, Some(20.0));
        assert_eq!(timed[0].sing_end, 17.5);

        // A manual override past the line's own end must still be clamped,
        // just like the automatic estimate is.
        lines[0].sing_end_override = Some(25.0);
        let timed = resolve_timing(&lines, Some(20.0));
        assert_eq!(timed[0].sing_end, 20.0);
    }

    #[test]
    fn sing_end_override_defaults_to_none() {
        let line = LyricLine::new("a b c");
        assert_eq!(line.sing_end_override, None);
    }

    #[test]
    fn hide_upcoming_lines_only_during_a_real_break() {
        // Short line, huge window -> long break -> hide once singing's done.
        let long_gap = TimedLine::new("hi".into(), 0.0, 20.0, Singer::Male);
        assert!(!hide_upcoming_lines(&long_gap, 0.0)); // still singing
        assert!(!hide_upcoming_lines(&long_gap, long_gap.sing_end - 0.01)); // just before done
        assert!(hide_upcoming_lines(&long_gap, long_gap.sing_end)); // done, break starts
        assert!(hide_upcoming_lines(&long_gap, 19.9)); // still in the break

        // Tight line, no meaningful gap -> never hide, even once "sung".
        let tight = TimedLine::new("hi there".into(), 0.0, 2.0, Singer::Male);
        assert!(!hide_upcoming_lines(&tight, tight.sing_end));
        assert!(!hide_upcoming_lines(&tight, 2.0));
    }

    #[test]
    fn blank_sung_lines_waits_for_linger_then_clears_until_countdown() {
        // 30s gap: sing_end is at 2.0 (2 short words), so cd_start is at
        // end-4.0. Linger keeps the sung line up for SUNG_LINGER_SECS past
        // sing_end, then it should blank until the countdown begins.
        let line = TimedLine::new("hi there".into(), 0.0, 30.0, Singer::Male);
        let cd_start = countdown_window(&line).unwrap().0;
        assert!(!blank_sung_lines(&line, line.sing_end)); // just finished, still lingering
        assert!(!blank_sung_lines(
            &line,
            line.sing_end + SUNG_LINGER_SECS - 0.01
        ));
        assert!(blank_sung_lines(&line, line.sing_end + SUNG_LINGER_SECS)); // linger's up
        assert!(blank_sung_lines(&line, cd_start - 0.01)); // still blank right up to the dots
        assert!(blank_sung_lines(&line, cd_start)); // dots are on; sung line stays cleared

        // Short/no gap -> never blank.
        let tight = TimedLine::new("hi there".into(), 0.0, 2.0, Singer::Male);
        assert!(!blank_sung_lines(&tight, tight.sing_end));
        assert!(!blank_sung_lines(&tight, 2.0));
    }

    #[test]
    fn word_end_override_lets_highlight_pause_before_next_word() {
        let mut lines = vec![LyricLine::new("one two")];
        lines[0].start = Some(0.0);
        // Force a wide-open window so "two" would normally start much later
        // than "one" finishes being highlighted by character count alone.
        let timed_before = resolve_timing(&lines, Some(20.0));
        let words_before = word_timings(&timed_before[0]);
        let one_start = words_before[0].highlight_at;
        let two_start = words_before[1].highlight_at;
        assert!(
            two_start > one_start + 0.1,
            "test needs a real gap to hold across"
        );

        // Without an override, "one"'s own held_until already equals "two"'s
        // start (a continuous wipe) - the fraction should keep climbing
        // smoothly right up to when "two" begins.
        let mid = one_start + (two_start - one_start) / 2.0;
        let frac_no_override = current_line_wipe_fraction(&timed_before[0], mid);
        assert!(frac_no_override > 0.0 && frac_no_override < 0.5);

        // With an explicit end well before "two" starts, the wipe should
        // reach full for "one" and then hold there until "two" begins.
        let one_end = one_start + (two_start - one_start) * 0.25;
        lines[0].word_end_overrides[0] = Some(one_end);
        let timed_after = resolve_timing(&lines, Some(20.0));
        let words_after = word_timings(&timed_after[0]);
        assert_eq!(words_after[0].held_until, one_end);
        assert!(words_after[0].end_is_manual);

        let frac_at_end = current_line_wipe_fraction(&timed_after[0], one_end);
        let frac_holding = current_line_wipe_fraction(&timed_after[0], mid); // past one_end, before two_start
        assert!(
            (frac_at_end - frac_holding).abs() < 1e-6,
            "should hold at the same fraction"
        );
        assert!(
            frac_at_end > frac_no_override,
            "should reach 'one' full sooner than the default"
        );
    }

    #[test]
    fn timecode_format_and_parse_round_trip() {
        assert_eq!(format_timecode(0.0), "00:00.00");
        assert_eq!(format_timecode(65.5), "01:05.50");
        assert_eq!(format_timecode(-1.0), "00:00.00");
        assert_eq!(format_timecode(f64::NAN), "00:00.00");

        assert_eq!(parse_timecode("01:05.50"), Some(65.5));
        assert_eq!(parse_timecode("1:05.5"), Some(65.5));
        assert_eq!(parse_timecode("5.5"), Some(5.5));
        assert_eq!(parse_timecode("5"), Some(5.0));
        assert_eq!(parse_timecode("  01:05.50  "), Some(65.5));
        assert_eq!(parse_timecode(""), None);
        assert_eq!(parse_timecode("abc"), None);
        assert_eq!(parse_timecode("-1:00"), None);

        for secs in [0.0, 1.23, 65.5, 3599.99] {
            let formatted = format_timecode(secs);
            let parsed = parse_timecode(&formatted).unwrap();
            assert!(
                (parsed - secs).abs() < 0.005,
                "{secs} round-tripped to {parsed}"
            );
        }
    }

    #[test]
    fn check_start_change_rejects_negative_and_own_end_crossing() {
        let mut lines = vec![LyricLine::new("a")];
        lines[0].sing_end_override = Some(5.0);
        assert!(check_start_change(&lines, 0, -1.0).is_err());
        assert!(check_start_change(&lines, 0, 5.0).is_err()); // not strictly before own end
        assert!(check_start_change(&lines, 0, 4.9).is_ok());
    }

    #[test]
    fn check_start_change_rejects_overlap_with_previous_lines_explicit_end() {
        let mut lines = vec![LyricLine::new("a"), LyricLine::new("b")];
        lines[0].start = Some(0.0);
        lines[0].sing_end_override = Some(3.0);
        assert!(check_start_change(&lines, 1, 2.9).is_err());
        assert!(check_start_change(&lines, 1, 3.0).is_ok());
    }

    #[test]
    fn check_start_change_allows_anything_when_previous_end_is_only_estimated() {
        // No explicit sing_end_override on the previous line - shouldn't be
        // treated as a hard wall, since the estimate will keep shifting as
        // more lines get timed.
        let mut lines = vec![LyricLine::new("a"), LyricLine::new("b")];
        lines[0].start = Some(0.0);
        assert!(check_start_change(&lines, 1, 0.01).is_ok());
    }

    #[test]
    fn check_end_change_requires_a_start_and_must_come_after_it() {
        let lines = vec![LyricLine::new("a")];
        assert!(check_end_change(&lines, 0, 5.0).is_err()); // no start yet

        let mut lines = vec![LyricLine::new("a")];
        lines[0].start = Some(5.0);
        assert!(check_end_change(&lines, 0, 5.0).is_err()); // not strictly after
        assert!(check_end_change(&lines, 0, 5.1).is_ok());
    }

    #[test]
    fn check_end_change_rejects_overlap_with_next_lines_start() {
        let mut lines = vec![LyricLine::new("a"), LyricLine::new("b")];
        lines[0].start = Some(0.0);
        lines[1].start = Some(5.0);
        assert!(check_end_change(&lines, 0, 5.1).is_err());
        assert!(check_end_change(&lines, 0, 5.0).is_ok());
    }
}
