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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
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

fn estimate_sing_duration(text: &str, window: f64) -> f64 {
    let word_count = text.split_whitespace().count().max(1);
    let est = (word_count as f64 * SECONDS_PER_WORD).max(MIN_SING_DURATION);
    est.min(window.max(0.0))
}

#[derive(Clone, Debug, PartialEq)]
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
            starts_new_block: true,
        }
    }

    #[allow(dead_code)]
    pub fn words(&self) -> Vec<&str> {
        self.text.split_whitespace().collect()
    }
}

/// Parse raw pasted lyrics text into lines, one [`LyricLine`] per non-empty
/// input line. Blank lines are dropped (they'd otherwise become an empty,
/// untimeable line) but their *position* is preserved: a line immediately
/// following one or more blank lines is marked as starting a new verse
/// block (see [`LyricLine::starts_new_block`]), which the video export uses
/// to group lines into on-screen blocks - this is why leaving blank lines
/// between verses/stanzas in your pasted lyrics is worth doing, even though
/// the blank lines themselves don't become timeable entries.
pub fn parse_pasted_lyrics(raw: &str) -> Vec<LyricLine> {
    let mut result = Vec::new();
    let mut pending_blank = false;
    for raw_line in raw.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
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
    /// See [`LyricLine::starts_new_block`].
    pub starts_new_block: bool,
}

impl TimedLine {
    #[allow(dead_code)]
    pub fn new(text: String, start: f64, end: f64, singer: Singer) -> Self {
        Self::with_overrides(text, start, end, singer, Vec::new(), true)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_overrides(
        text: String,
        start: f64,
        end: f64,
        singer: Singer,
        word_overrides: Vec<Option<f64>>,
        starts_new_block: bool,
    ) -> Self {
        let window = (end - start).max(0.0);
        let sing_end = (start + estimate_sing_duration(&text, window)).clamp(start, end);
        Self { text, start, end, sing_end, singer, word_overrides, starts_new_block }
    }
}

/// Resolve start/end windows for every line. Requires every line to already
/// have a `start` set (caller should validate this first). Lines are sorted
/// by start time. The final line's end is `total_duration` (or `start + 4.0`
/// if `total_duration` is unknown / shorter than that).
pub fn resolve_timing(lines: &[LyricLine], total_duration: Option<f64>) -> Vec<TimedLine> {
    let mut sorted: Vec<(f64, &str, Singer, &[Option<f64>], bool)> = lines
        .iter()
        .filter_map(|l| l.start.map(|s| (s, l.text.as_str(), l.singer, l.word_overrides.as_slice(), l.starts_new_block)))
        .collect();
    sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    let mut out = Vec::with_capacity(sorted.len());
    for i in 0..sorted.len() {
        let (start, text, singer, overrides, starts_new_block) = sorted[i];
        let end = if i + 1 < sorted.len() {
            sorted[i + 1].0
        } else {
            match total_duration {
                Some(d) if d > start => d,
                _ => start + 4.0,
            }
        };
        out.push(TimedLine::with_overrides(text.to_string(), start, end, singer, overrides.to_vec(), starts_new_block));
    }
    out
}

/// A word within a line, with its derived highlight-start time (proportional
/// to its position within the line's *estimated singing window*, weighted
/// by character count) - or the user's manually tapped time, if set.
pub struct TimedWord<'a> {
    #[allow(dead_code)]
    pub text: &'a str,
    pub highlight_at: f64,
    pub is_manual: bool,
}

/// Split a timed line into words, each with a derived highlight time
/// spread across `[line.start, line.sing_end)`, proportional to cumulative
/// character count - except where the user has manually overridden a
/// specific word's time, which takes precedence.
pub fn word_timings(line: &TimedLine) -> Vec<TimedWord<'_>> {
    let words: Vec<&str> = line.text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let total_chars: usize = words.iter().map(|w| w.chars().count()).sum::<usize>().max(1);
    let duration = (line.sing_end - line.start).max(0.05);

    let mut out = Vec::with_capacity(words.len());
    let mut chars_so_far = 0usize;
    for (i, w) in words.iter().enumerate() {
        let frac = chars_so_far as f64 / total_chars as f64;
        let auto_time = line.start + duration * frac;
        let manual = line.word_overrides.get(i).copied().flatten();
        out.push(TimedWord {
            text: w,
            highlight_at: manual.unwrap_or(auto_time),
            is_manual: manual.is_some(),
        });
        chars_so_far += w.chars().count();
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
/// *anchored* to each word's own timestamp (from [`word_timings`]) for
/// accuracy - reaching the right fraction at the right moment for each word
/// - but linearly interpolates between those anchors instead of jumping, so
/// the wipe moves continuously through every word's letters (and through a
/// single word held for several seconds) rather than only snapping at word
/// boundaries.
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
        let word_end = words.get(i + 1).map(|w| w.highlight_at).unwrap_or(line.sing_end.max(word_start + 0.05));
        let frac_start = offset as f32 / char_count;
        let frac_end = (offset + len) as f32 / char_count;
        if t < word_end || i + 1 == words.len() {
            let dur = (word_end - word_start).max(0.05);
            let local = ((t - word_start) / dur).clamp(0.0, 1.0) as f32;
            return frac_start + (frac_end - frac_start) * local;
        }
    }
    1.0
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
    fn resolve_timing_chains_end_to_next_start() {
        let mut lines = vec![LyricLine::new("a"), LyricLine::new("b"), LyricLine::new("c")];
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
        let mut lines = vec![LyricLine::new("a"), LyricLine::new("skip me"), LyricLine::new("c")];
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
}
