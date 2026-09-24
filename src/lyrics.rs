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
    /// No voice manually assigned - every line starts out this way. Has its
    /// own customizable color pair (defaulting to the same color as `Male`,
    /// but independently adjustable, e.g. to match a background image/
    /// video's palette) - see [`crate::export::Palette`]/
    /// [`crate::video::VideoPalette`].
    #[default]
    Default,
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
            Self::Default => "Default",
            Self::Male => "Male",
            Self::Female => "Female",
            Self::Duet => "Duet",
            Self::Screaming => "Screaming",
        }
    }

    /// Every variant, in a fixed display order - used both to build the
    /// color legend (see [`singer_legend`]) and, in `main.rs`, to build the
    /// timing table's bulk "set voice for selection" buttons from the same
    /// single list the per-line dropdown's options come from.
    pub const ALL: [Singer; 5] = [
        Self::Default,
        Self::Male,
        Self::Female,
        Self::Duet,
        Self::Screaming,
    ];
}

/// Per-line override for whether the "get ready" countdown indicator shows
/// up before this line starts - see [`countdown_window`]/[`LyricLine::countdown_mode`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum CountdownMode {
    /// Show the countdown only if the automatic gap threshold
    /// ([`TimingSettings::countdown_gap_threshold`]) says to - the normal,
    /// original behavior.
    #[default]
    Auto,
    /// Always show the countdown before this line, even if the gap is
    /// shorter than the automatic threshold - clamped the same way a short
    /// `Auto` gap already is (the dots compress to fit whatever room is
    /// actually there; a gap of zero or less shows nothing, since there's
    /// no room for anything).
    Force,
    /// Never show the countdown before this line, even if the gap is long
    /// enough that `Auto` would otherwise trigger it.
    Suppress,
}

impl CountdownMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Force => "Force",
            Self::Suppress => "Suppress",
        }
    }

    pub const ALL: [CountdownMode; 3] = [Self::Auto, Self::Force, Self::Suppress];
}

/// The distinct voices actually used across `lines`, in a fixed
/// Default/Male/Female/Duet/Screaming order regardless of which order they
/// first appear in the song. Used to build a color-legend on the intro
/// screen so singers can see which color means what before the song
/// starts. Returns an empty list for a song with only one (or zero)
/// distinct voice, since a legend only earns its place once more than one
/// color is actually in play - e.g. a duet, or a song with a dedicated
/// screaming section - which also means an untouched song (every line
/// still at `Default`) never gets a legend.
pub fn singer_legend(lines: &[TimedLine]) -> Vec<Singer> {
    let used: Vec<Singer> = Singer::ALL
        .into_iter()
        .filter(|s| lines.iter().any(|l| l.singer == *s))
        .collect();
    if used.len() > 1 {
        used
    } else {
        Vec::new()
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

/// User-configurable overrides for [`SECONDS_PER_WORD`], [`MIN_SING_DURATION`],
/// and [`COUNTDOWN_GAP_THRESHOLD`] - exposed in the app's "Timing" settings
/// panel and persisted per-project (see `project::ProjectFile::timing_settings`),
/// so a song that needs a different word-per-second pace or countdown
/// sensitivity doesn't need a source edit. `Default` is exactly this app's
/// own long-standing constants above, so a project that never touches this
/// panel gets byte-identical output to before this setting existed.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TimingSettings {
    /// See [`SECONDS_PER_WORD`].
    pub seconds_per_word: f64,
    /// See [`MIN_SING_DURATION`].
    pub min_sing_duration: f64,
    /// See [`COUNTDOWN_GAP_THRESHOLD`].
    pub countdown_gap_threshold: f64,
}

impl Default for TimingSettings {
    fn default() -> Self {
        Self {
            seconds_per_word: SECONDS_PER_WORD,
            min_sing_duration: MIN_SING_DURATION,
            countdown_gap_threshold: COUNTDOWN_GAP_THRESHOLD,
        }
    }
}
/// The countdown indicator occupies this many seconds immediately before
/// the next line begins.
pub const COUNTDOWN_LEAD_SECS: f64 = 4.0;

/// How long an already-sung line (and any earlier lines still shown
/// highlighted in the same block) lingers on screen after singing ends,
/// during a break long enough to trigger the countdown indicator. Past
/// this point the display goes blank (until the countdown dots appear)
/// instead of leaving finished lyrics sitting there for the whole break.
pub const SUNG_LINGER_SECS: f64 = 5.0;

fn estimate_sing_duration(text: &str, window: f64, settings: &TimingSettings) -> f64 {
    let word_count = text.split_whitespace().count().max(1);
    let est = (word_count as f64 * settings.seconds_per_word).max(settings.min_sing_duration);
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
    /// A second vocalist echoing/repeating part of this line while it's
    /// still being sung (typically the last word or phrase) - `None` for
    /// every line by default; added via the "+ Backing vocal" button.
    /// Absent from every project saved before this existed, via
    /// `#[serde(default)]`. See [`BackingVocal`] for why its own timing is
    /// bounded within this line's own window rather than being a fully
    /// independent line in its own right.
    #[serde(default)]
    pub backing_vocal: Option<BackingVocal>,
    /// Manual override for whether the "get ready" countdown shows up
    /// before *this* line - see [`CountdownMode`]. `Auto` (the default,
    /// via `#[serde(default)]`) for every line/project saved before this
    /// existed, matching the automatic-only behavior they already had.
    #[serde(default)]
    pub countdown_mode: CountdownMode,
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
            backing_vocal: None,
            countdown_mode: CountdownMode::default(),
        }
    }

    pub fn words(&self) -> Vec<&str> {
        self.text.split_whitespace().collect()
    }
}

/// A secondary vocalist's line, overlapping part of its host [`LyricLine`]
/// while the host is still being sung (typically the last word or phrase of
/// the host line, echoed or answered) - not a full top-level line of its
/// own, but an attachment on the line it echoes.
///
/// Deliberately embedded here rather than represented as a second
/// independent entry in the main line list, for two reasons: it sidesteps
/// needing a stable cross-line reference that would have to survive
/// re-parsing (`merge_reparsed_lyrics` already matches *host* lines by
/// content; a backing vocal attached to a matched host just comes along for
/// free, no separate identity/matching scheme needed), and its timing is
/// constrained to fall entirely within its host's own `[start, end)` window,
/// which is what keeps `resolve_timing`'s sequential, non-overlapping
/// `Vec<TimedLine>` unchanged: a backing vocal is purely an extra thing its
/// host's own rendering step draws during its own window, never a second
/// independent timeline entry. If a real use case ever needs an echo that
/// outlasts its host line, that's a deliberate future relaxation, not
/// something built speculatively now.
///
/// Reuses [`Singer`] for its own color rather than a dedicated "backing"
/// voice category, so it can be colored distinctly from its host (e.g. host
/// = Male, backing = Female) using exactly the palette slots that already
/// exist - no new CLUT budget needed in the `.cdg` export.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BackingVocal {
    pub text: String,
    /// Start time in seconds - must fall within the host line's own
    /// `[start, end)` once resolved (see [`check_backing_vocal_start`]).
    /// `None` means "not yet tapped"; until it's set, this backing vocal
    /// doesn't render at all (nothing to time it by), the same way an
    /// untimed [`LyricLine`] doesn't appear in [`resolve_timing`]'s output.
    pub start: Option<f64>,
    /// End time in seconds. `None` means "use the host line's own end" as a
    /// placeholder once `start` is set, the same "graceful default, refine
    /// later" behavior [`LyricLine::sing_end_override`] already has.
    pub end: Option<f64>,
    pub singer: Singer,
    /// Same shape/meaning as [`LyricLine::word_overrides`], scoped to this
    /// backing vocal's own words.
    pub word_overrides: Vec<Option<f64>>,
    /// Same shape/meaning as [`LyricLine::word_end_overrides`].
    pub word_end_overrides: Vec<Option<f64>>,
}

impl BackingVocal {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let word_count = text.split_whitespace().count();
        Self {
            text,
            start: None,
            end: None,
            singer: Singer::default(),
            word_overrides: vec![None; word_count],
            word_end_overrides: vec![None; word_count],
        }
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

/// Tallies what [`merge_reparsed_lyrics`] did, so the caller can tell the
/// user something more useful than just a new line count.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReparseReport {
    /// Lines whose text didn't change at all - every field carried over.
    pub unchanged: usize,
    /// Lines matched to an edited old line with the *same* word count -
    /// line-level timing kept, and per-word timing kept for whichever
    /// words didn't change.
    pub reworded_same_word_count: usize,
    /// Lines matched to an edited old line with a *different* word count -
    /// line-level timing kept, but word-level timing reset (there's no
    /// sound way to carry it over when the words themselves don't line up).
    pub reworded_word_count_changed: usize,
    /// Lines with no old counterpart at all - start untimed, as normal.
    pub added: usize,
    /// Old lines with no counterpart in the new text - their timing is
    /// simply gone, since there's nothing left to attach it to.
    pub removed: usize,
}

impl ReparseReport {
    /// True if every line's text was already exactly what it is now - i.e.
    /// re-parsing was a pure no-op as far as timing is concerned.
    pub fn is_fully_unchanged(&self, total_lines: usize) -> bool {
        self.unchanged == total_lines
            && self.reworded_same_word_count == 0
            && self.reworded_word_count_changed == 0
            && self.added == 0
            && self.removed == 0
    }

    /// A short, human-readable summary for the status bar.
    pub fn summarize(&self, total_lines: usize) -> String {
        if self.is_fully_unchanged(total_lines) {
            return format!(
                "Parsed {total_lines} line(s) - no text changed, all existing timing kept."
            );
        }
        let mut parts = Vec::new();
        if self.unchanged > 0 {
            parts.push(format!("{} unchanged", self.unchanged));
        }
        if self.reworded_same_word_count > 0 {
            parts.push(format!(
                "{} reworded (timing kept)",
                self.reworded_same_word_count
            ));
        }
        if self.reworded_word_count_changed > 0 {
            parts.push(format!(
                "{} reworded with a different word count (line timing kept, word timing reset)",
                self.reworded_word_count_changed
            ));
        }
        if self.added > 0 {
            parts.push(format!("{} new", self.added));
        }
        if self.removed > 0 {
            parts.push(format!("{} removed", self.removed));
        }
        format!("Parsed {total_lines} line(s): {}.", parts.join(", "))
    }
}

/// Finds the longest common subsequence of lines between `old` and `new`,
/// matched by exact text equality, as a list of `(old_index, new_index)`
/// pairs strictly increasing in both - i.e. the largest set of lines that
/// appear unmodified, in the same relative order, in both. This is what
/// [`merge_reparsed_lyrics`] treats as "definitely the same line, unedited",
/// with everything else falling into a gap between two such anchors (or
/// before the first/after the last) that gets a best-effort positional
/// pairing.
///
/// Matching by *content* rather than list position is what makes an
/// insertion/deletion earlier in the song not smear every later line's
/// timing forward/backward by one slot, and what keeps a duplicated line
/// (e.g. a chorus repeated twice) from having its two occurrences' timing
/// swapped: since matched pairs must be increasing in both indices, the
/// first old occurrence can only ever match the first surviving new
/// occurrence, never the second.
///
/// O(n*m) time and space - fine for realistic lyric line counts (tens to a
/// few hundred), not chosen with e.g. a full song's worth of KOK-imported
/// word-level fragments in mind.
fn lcs_matches(old: &[LyricLine], new: &[LyricLine]) -> Vec<(usize, usize)> {
    let n = old.len();
    let m = new.len();
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if old[i].text == new[j].text {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut pairs = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if old[i].text == new[j].text {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// Merges `old`'s timing into `new` for a pair of lines matched within the
/// same local gap (see [`merge_reparsed_lyrics`]) - i.e. treated as an
/// edited version of the same line, not a brand new one, even though the
/// text itself differs. Line-level timing (`start`, `sing_end_override`,
/// `singer`) is always kept - a typo fix or reworded phrase doesn't change
/// *when* the line is sung. Word-level timing is only kept where it can
/// still mean something: if the word count matches, each word slot whose
/// text is unchanged at that position keeps its override; every other slot
/// (word count changed, or that specific word changed) resets to the
/// automatic estimate, same as a freshly-parsed word.
///
/// Returns the merged line and whether the word count matched, so the
/// caller can tally which case applied (see [`ReparseReport`]).
fn merge_edited_line(old: &LyricLine, new: &LyricLine) -> (LyricLine, bool) {
    let mut merged = new.clone();
    merged.start = old.start;
    merged.sing_end_override = old.sing_end_override;
    merged.singer = old.singer;
    // A backing vocal is attached to the host line's *identity*, not to its
    // exact word count/text - a reworded host line (typo fix, rephrasing)
    // keeps whatever backing vocal it had, same as it keeps its own timing.
    merged.backing_vocal = old.backing_vocal.clone();

    let old_words: Vec<&str> = old.text.split_whitespace().collect();
    let new_words: Vec<&str> = new.text.split_whitespace().collect();
    let same_word_count = old_words.len() == new_words.len();
    if same_word_count {
        for (i, (&ow, &nw)) in old_words.iter().zip(new_words.iter()).enumerate() {
            if ow == nw {
                merged.word_overrides[i] = old.word_overrides.get(i).copied().flatten();
                merged.word_end_overrides[i] = old.word_end_overrides.get(i).copied().flatten();
            }
            // A word that itself changed keeps `merged`'s fresh `None` from
            // `new.clone()` above - carrying over a manually-tapped time for
            // a word that no longer exists would silently mistime whatever
            // word replaced it.
        }
    }
    // Different word count: nothing above applies, so `merged` keeps the
    // all-`None` word_overrides/word_end_overrides it already got from
    // `new.clone()` - there's no sound way to line up two word lists of
    // different lengths without risking attaching a time to the wrong word.
    (merged, same_word_count)
}

/// Re-parses `raw` into a fresh line list, the same way [`parse_pasted_lyrics`]
/// always has, but carries over as much of `old`'s existing timing as it
/// safely can instead of discarding it wholesale - re-parsing after fixing
/// a typo (or inserting/reordering a line) used to silently wipe every
/// line's timing, which is a serious problem on any song that was already
/// tapped/tuned.
///
/// Matching strategy (see [`lcs_matches`] for the "why"): lines with
/// *exactly* unchanged text, in the same relative order, are matched first
/// via a longest-common-subsequence pass - these keep every field
/// untouched. Whatever's left (edited/inserted/deleted lines) falls between
/// two such matches (or before the first/after the last); within one such
/// gap, remaining old and new lines are paired positionally in order (the
/// gap is, by construction, a small localized region of actual change, so
/// "the Nth old line in this gap corresponds to the Nth new line in this
/// gap" is a reasonable assumption without more information) - see
/// [`merge_edited_line`] for what a paired-but-different-text line keeps.
/// A leftover old line with no new counterpart in its gap is dropped
/// (nothing left to attach its timing to); a leftover new line with no old
/// counterpart starts untimed, same as any brand new line.
pub fn merge_reparsed_lyrics(old: &[LyricLine], raw: &str) -> (Vec<LyricLine>, ReparseReport) {
    let new = parse_pasted_lyrics(raw);
    let anchors = lcs_matches(old, &new);

    let mut result = Vec::with_capacity(new.len());
    let mut report = ReparseReport::default();
    let mut old_pos = 0usize;
    let mut new_pos = 0usize;

    for seg in 0..=anchors.len() {
        let (old_end, new_end) = anchors.get(seg).copied().unwrap_or((old.len(), new.len()));

        // The gap before this anchor (or, on the final pass, before the
        // end of both lists): best-effort positional pairing between
        // whatever's left unmatched here.
        let gap_old = &old[old_pos..old_end];
        let gap_new = &new[new_pos..new_end];
        let paired = gap_old.len().min(gap_new.len());
        for k in 0..paired {
            let (merged, same_word_count) = merge_edited_line(&gap_old[k], &gap_new[k]);
            if same_word_count {
                report.reworded_same_word_count += 1;
            } else {
                report.reworded_word_count_changed += 1;
            }
            result.push(merged);
        }
        result.extend_from_slice(&gap_new[paired..]);
        report.added += gap_new.len() - paired;
        report.removed += gap_old.len() - paired;

        // The anchor itself (exact text match), if this wasn't the final
        // (sentinel) pass - carry every field over untouched.
        if let Some(&(oi, nj)) = anchors.get(seg) {
            let mut kept = new[nj].clone();
            kept.start = old[oi].start;
            kept.sing_end_override = old[oi].sing_end_override;
            kept.singer = old[oi].singer;
            kept.word_overrides = old[oi].word_overrides.clone();
            kept.word_end_overrides = old[oi].word_end_overrides.clone();
            kept.backing_vocal = old[oi].backing_vocal.clone();
            result.push(kept);
            report.unchanged += 1;
            old_pos = oi + 1;
            new_pos = nj + 1;
        } else {
            old_pos = old_end;
            new_pos = new_end;
        }
    }

    (result, report)
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

/// Checks whether setting a backing vocal's `start` to `new_start` is
/// valid: must fall within its host line's own resolved
/// `[host_start, host_end)` window, since a backing vocal's timing is
/// bounded to its host's (see [`BackingVocal`]) rather than checked against
/// the main line list's own neighbor rules.
pub fn check_backing_vocal_start(
    new_start: f64,
    host_start: f64,
    host_end: f64,
) -> Result<(), String> {
    if !new_start.is_finite() || new_start < host_start {
        return Err(format!(
            "Start ({}) can't be before the main line starts ({}).",
            format_timecode(new_start),
            format_timecode(host_start)
        ));
    }
    if new_start >= host_end {
        return Err(format!(
            "Start ({}) must be before the main line ends ({}).",
            format_timecode(new_start),
            format_timecode(host_end)
        ));
    }
    Ok(())
}

/// Checks whether setting a backing vocal's `end` to `new_end` is valid -
/// after its own `backing_start`, and not after the host line's own end.
pub fn check_backing_vocal_end(
    new_end: f64,
    backing_start: f64,
    host_end: f64,
) -> Result<(), String> {
    if !new_end.is_finite() || new_end <= backing_start {
        return Err(format!(
            "End ({}) must be after the backing vocal's own start ({}).",
            format_timecode(new_end),
            format_timecode(backing_start)
        ));
    }
    if new_end > host_end {
        return Err(format!(
            "End ({}) can't be after the main line ends ({}).",
            format_timecode(new_end),
            format_timecode(host_end)
        ));
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
    /// Resolved from [`LyricLine::backing_vocal`] - `None` either because
    /// the host line has no backing vocal at all, or because it has one but
    /// its `start` hasn't been tapped yet (nothing to time it by, so it
    /// doesn't render - see [`BackingVocal::start`]).
    pub backing_vocal: Option<TimedBackingVocal>,
    /// See [`LyricLine::countdown_mode`].
    pub countdown_mode: CountdownMode,
}

impl TimedLine {
    #[allow(dead_code)]
    pub fn new(text: String, start: f64, end: f64, singer: Singer) -> Self {
        let mut line = LyricLine::new(text);
        line.singer = singer;
        Self::from_line(&line, start, end)
    }

    /// Same as [`Self::from_line_with_settings`], using [`TimingSettings::default`] -
    /// kept as its own entry point so the many existing callers/tests that
    /// don't care about custom timing settings don't all need updating just
    /// because this setting now exists.
    pub fn from_line(line: &LyricLine, start: f64, end: f64) -> Self {
        Self::from_line_with_settings(line, start, end, &TimingSettings::default())
    }

    /// Build a resolved [start, end) window for `line`, carrying over its
    /// word/end/singer overrides. Used by [`resolve_timing_with_settings`]
    /// and by the live preview (which needs the same resolution but also
    /// wants to know which original `LyricLine` each result came from).
    pub fn from_line_with_settings(
        line: &LyricLine,
        start: f64,
        end: f64,
        settings: &TimingSettings,
    ) -> Self {
        let window = (end - start).max(0.0);
        let sing_end = match line.sing_end_override {
            Some(manual) => manual.clamp(start, end),
            None => {
                (start + estimate_sing_duration(&line.text, window, settings)).clamp(start, end)
            }
        };
        let backing_vocal = line.backing_vocal.as_ref().and_then(|bv| {
            let bv_start = bv.start?.clamp(start, end);
            // No end tapped yet -> the host's own end is a reasonable
            // placeholder, the same "graceful default, refine later"
            // behavior an untapped line end already has.
            let bv_end = bv
                .end
                .unwrap_or(end)
                .clamp(bv_start, end)
                .max(bv_start + 0.05);
            Some(TimedBackingVocal {
                text: bv.text.clone(),
                start: bv_start,
                end: bv_end,
                singer: bv.singer,
                word_overrides: bv.word_overrides.clone(),
                word_end_overrides: bv.word_end_overrides.clone(),
            })
        });
        Self {
            text: line.text.clone(),
            start,
            end,
            sing_end,
            singer: line.singer,
            word_overrides: line.word_overrides.clone(),
            word_end_overrides: line.word_end_overrides.clone(),
            starts_new_block: line.starts_new_block,
            backing_vocal,
            countdown_mode: line.countdown_mode,
        }
    }
}

/// The resolved (always-timed) counterpart to [`BackingVocal`] - see
/// [`TimedLine::backing_vocal`].
#[derive(Clone, Debug, PartialEq)]
pub struct TimedBackingVocal {
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub singer: Singer,
    pub word_overrides: Vec<Option<f64>>,
    pub word_end_overrides: Vec<Option<f64>>,
}

/// Same as [`resolve_timing_with_settings`], using [`TimingSettings::default`] -
/// kept as its own entry point so the many existing callers/tests that
/// don't care about custom timing settings don't all need updating just
/// because this setting now exists. Real production code calls
/// `resolve_timing_with_settings` directly (see `main.rs`), so this is only
/// reached by tests in a normal (non-test) build - same reasoning as
/// `TimedLine::new`'s own `#[allow(dead_code)]` just above.
#[allow(dead_code)]
pub fn resolve_timing(lines: &[LyricLine], total_duration: Option<f64>) -> Vec<TimedLine> {
    resolve_timing_with_settings(lines, total_duration, &TimingSettings::default())
}

/// Resolve start/end windows for every line. Requires every line to already
/// have a `start` set (caller should validate this first). Lines are sorted
/// by start time. The final line's end is `total_duration` (or `start + 4.0`
/// if `total_duration` is unknown / shorter than that).
pub fn resolve_timing_with_settings(
    lines: &[LyricLine],
    total_duration: Option<f64>,
    settings: &TimingSettings,
) -> Vec<TimedLine> {
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
        out.push(TimedLine::from_line_with_settings(
            line, start, end, settings,
        ));
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
    resolve_word_timings(
        &line.text,
        line.start,
        line.sing_end,
        &line.word_overrides,
        &line.word_end_overrides,
    )
}

/// Same as [`word_timings`], for a backing vocal's own (much shorter, and
/// bounded within its host's window) text/timing instead of a host line's.
pub fn backing_vocal_word_timings(bv: &TimedBackingVocal) -> Vec<TimedWord<'_>> {
    resolve_word_timings(
        &bv.text,
        bv.start,
        bv.end,
        &bv.word_overrides,
        &bv.word_end_overrides,
    )
}

/// The shared math behind [`word_timings`]/[`backing_vocal_word_timings`] -
/// spreads `text`'s words across `[start, sing_end)`, proportional to
/// cumulative character count, except where `word_overrides`/
/// `word_end_overrides` manually pin a specific word's start and/or end.
fn resolve_word_timings<'a>(
    text: &'a str,
    start: f64,
    sing_end: f64,
    word_overrides: &[Option<f64>],
    word_end_overrides: &[Option<f64>],
) -> Vec<TimedWord<'a>> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let total_chars: usize = words
        .iter()
        .map(|w| w.chars().count())
        .sum::<usize>()
        .max(1);
    let duration = (sing_end - start).max(0.05);

    // Resolve every word's *start* first (manual override, or the automatic
    // character-weighted estimate), so each word's automatic *end* can then
    // be defined as "the next word's resolved start" - matching the
    // historical continuous-wipe behavior whenever no end is manually set.
    let mut resolved_starts = Vec::with_capacity(words.len());
    let mut chars_so_far = 0usize;
    for (i, w) in words.iter().enumerate() {
        let frac = chars_so_far as f64 / total_chars as f64;
        let auto_start = start + duration * frac;
        let manual_start = word_overrides.get(i).copied().flatten();
        resolved_starts.push(manual_start.unwrap_or(auto_start));
        chars_so_far += w.chars().count();
    }

    let mut out = Vec::with_capacity(words.len());
    for (i, w) in words.iter().enumerate() {
        let word_start = resolved_starts[i];
        let manual_start = word_overrides.get(i).copied().flatten();
        let default_end = resolved_starts.get(i + 1).copied().unwrap_or(sing_end);
        let manual_end = word_end_overrides.get(i).copied().flatten();
        out.push(TimedWord {
            text: w,
            highlight_at: word_start,
            held_until: manual_end.unwrap_or(default_end).max(word_start),
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
    resolve_wipe_fraction(&normalize_text(&line.text), &word_timings(line), t)
}

/// Same as [`current_line_wipe_fraction`], for a backing vocal's own
/// (much shorter) text/word timing.
pub fn backing_vocal_wipe_fraction(bv: &TimedBackingVocal, t: f64) -> f32 {
    resolve_wipe_fraction(
        &normalize_text(&bv.text),
        &backing_vocal_word_timings(bv),
        t,
    )
}

/// The shared math behind [`current_line_wipe_fraction`]/
/// [`backing_vocal_wipe_fraction`].
fn resolve_wipe_fraction(text: &str, words: &[TimedWord], t: f64) -> f32 {
    let char_count = text.chars().count().max(1) as f32;
    let spans = word_char_spans(text);
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
///
/// `mode` is the [`CountdownMode`] of whichever line begins at `next_start`
/// (the one the countdown announces) - `Suppress` always returns `None`
/// regardless of gap size, `Force` always returns a window as long as
/// there's *any* positive gap to work with (clamped exactly like a short
/// `Auto` gap already is - the dots compress to fit whatever room is
/// actually there), and `Auto` keeps the original threshold-based behavior.
pub fn countdown_window_between(
    after: f64,
    next_start: f64,
    mode: CountdownMode,
    settings: &TimingSettings,
) -> Option<(f64, f64)> {
    if mode == CountdownMode::Suppress {
        return None;
    }
    let leftover = next_start - after;
    let auto_triggers = leftover >= settings.countdown_gap_threshold;
    if leftover > 0.0 && (mode == CountdownMode::Force || auto_triggers) {
        let cd_start = (next_start - COUNTDOWN_LEAD_SECS).max(after);
        Some((cd_start, next_start))
    } else {
        None
    }
}

/// If a line has a long enough musical break before the *next* line starts,
/// returns the `(countdown_start, countdown_end)` window (in seconds) during
/// which a "get ready" countdown indicator should be shown. `countdown_end`
/// is always equal to `line.end` (i.e. the next line's start). `next_mode`
/// is the upcoming line's own [`CountdownMode`] - see
/// [`countdown_window_between`].
pub fn countdown_window(
    line: &TimedLine,
    next_mode: CountdownMode,
    settings: &TimingSettings,
) -> Option<(f64, f64)> {
    countdown_window_between(line.sing_end, line.end, next_mode, settings)
}

/// True once `line` is done being sung (`t >= line.sing_end`) and there's a
/// real musical break before the next line (i.e. [`countdown_window`] would
/// fire). Any not-yet-started lines still visible in the same on-screen
/// block should be hidden while this is true, instead of sitting on screen
/// for the whole break - the screen should read as "done, waiting" (then
/// the countdown indicator, then the next line), not show lyrics that are
/// still a break away. Shared by the video exporter, the CDG exporter, and
/// the live preview so all three agree on when to hide ahead-of-time lines.
pub fn hide_upcoming_lines(
    line: &TimedLine,
    t: f64,
    next_mode: CountdownMode,
    settings: &TimingSettings,
) -> bool {
    t >= line.sing_end && countdown_window(line, next_mode, settings).is_some()
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
pub fn blank_sung_lines(
    line: &TimedLine,
    t: f64,
    next_mode: CountdownMode,
    settings: &TimingSettings,
) -> bool {
    match countdown_window(line, next_mode, settings) {
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
        assert_eq!(lines[0].singer, Singer::Default); // default
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
    fn singer_legend_is_empty_when_only_one_voice_is_used() {
        let lines = vec![
            TimedLine::new("a".into(), 0.0, 1.0, Singer::Male),
            TimedLine::new("b".into(), 1.0, 2.0, Singer::Male),
        ];
        assert!(singer_legend(&lines).is_empty());
    }

    #[test]
    fn singer_legend_is_empty_for_no_lines() {
        assert!(singer_legend(&[]).is_empty());
    }

    #[test]
    fn singer_legend_is_empty_for_an_untouched_song() {
        // Nothing manually assigned - every line is still `Default`. Even
        // though that renders as `Male`, there's only one color in play, so
        // no legend should appear.
        let lines = vec![
            TimedLine::new("a".into(), 0.0, 1.0, Singer::Default),
            TimedLine::new("b".into(), 1.0, 2.0, Singer::Default),
        ];
        assert!(singer_legend(&lines).is_empty());
    }

    #[test]
    fn singer_legend_shows_up_once_any_line_is_set_away_from_default() {
        // A realistic duet: only the second voice's lines get manually
        // assigned, the rest are left at `Default`. The legend should
        // still appear, listing `Default` and `Female` - the two voices
        // actually on screen (each independently colorable).
        let lines = vec![
            TimedLine::new("his line".into(), 0.0, 1.0, Singer::Default),
            TimedLine::new("her line".into(), 1.0, 2.0, Singer::Female),
        ];
        assert_eq!(singer_legend(&lines), vec![Singer::Default, Singer::Female]);
    }

    #[test]
    fn singer_legend_lists_distinct_singers_in_a_fixed_order() {
        // Female appears before Male in the song, but the legend should
        // still list them in the fixed Male/Female/Duet/Screaming order.
        let lines = vec![
            TimedLine::new("a".into(), 0.0, 1.0, Singer::Female),
            TimedLine::new("b".into(), 1.0, 2.0, Singer::Male),
            TimedLine::new("c".into(), 2.0, 3.0, Singer::Female),
            TimedLine::new("d".into(), 3.0, 4.0, Singer::Screaming),
        ];
        assert_eq!(
            singer_legend(&lines),
            vec![Singer::Male, Singer::Female, Singer::Screaming]
        );
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
        let cd = countdown_window(&long_gap, CountdownMode::Auto, &TimingSettings::default());
        assert!(cd.is_some());
        let (start, end) = cd.unwrap();
        assert_eq!(end, 20.0);
        assert!(start < end);
        assert!(start >= long_gap.sing_end);

        // Tight line, no meaningful gap -> no countdown.
        let tight = TimedLine::new("hi there".into(), 0.0, 2.0, Singer::Male);
        assert!(
            countdown_window(&tight, CountdownMode::Auto, &TimingSettings::default()).is_none()
        );
    }

    #[test]
    fn timing_settings_default_matches_the_bare_constants() {
        // Phase 1's own acceptance criterion: a project that never touches
        // the settings panel must behave exactly as before this setting
        // existed.
        let settings = TimingSettings::default();
        assert_eq!(settings.seconds_per_word, SECONDS_PER_WORD);
        assert_eq!(settings.min_sing_duration, MIN_SING_DURATION);
        assert_eq!(settings.countdown_gap_threshold, COUNTDOWN_GAP_THRESHOLD);
    }

    #[test]
    fn custom_seconds_per_word_changes_the_sing_end_estimate() {
        let mut lines = vec![LyricLine::new("one two three four")];
        lines[0].start = Some(0.0);

        let default_timed =
            resolve_timing_with_settings(&lines, Some(20.0), &TimingSettings::default());
        let slower = TimingSettings {
            seconds_per_word: 2.0,
            ..TimingSettings::default()
        };
        let slower_timed = resolve_timing_with_settings(&lines, Some(20.0), &slower);

        // 4 words at 2.0s/word = 8.0s, well past the default 0.45s/word
        // estimate - and still clamped to the window like the default is.
        assert_eq!(slower_timed[0].sing_end, 8.0);
        assert!(slower_timed[0].sing_end > default_timed[0].sing_end);
    }

    #[test]
    fn custom_min_sing_duration_raises_the_floor_for_short_lines() {
        let mut lines = vec![LyricLine::new("hi")];
        lines[0].start = Some(0.0);

        let raised = TimingSettings {
            min_sing_duration: 5.0,
            ..TimingSettings::default()
        };
        let timed = resolve_timing_with_settings(&lines, Some(20.0), &raised);
        // 1 word at the default 0.45s/word pace would normally floor out at
        // the *default* MIN_SING_DURATION (1.2s) - the custom, higher floor
        // should win instead.
        assert_eq!(timed[0].sing_end, 5.0);
    }

    #[test]
    fn custom_countdown_gap_threshold_changes_whether_a_countdown_fires() {
        // A 4s gap: no countdown under the default 5s threshold, but should
        // trigger once the threshold is lowered below the gap's own size.
        let line = TimedLine::new("hi".into(), 0.0, 4.0 + MIN_SING_DURATION, Singer::Male);
        assert!(countdown_window(&line, CountdownMode::Auto, &TimingSettings::default()).is_none());

        let sensitive = TimingSettings {
            countdown_gap_threshold: 3.0,
            ..TimingSettings::default()
        };
        assert!(countdown_window(&line, CountdownMode::Auto, &sensitive).is_some());
    }

    #[test]
    fn countdown_mode_force_bypasses_the_gap_threshold() {
        // Same short gap as the "no countdown" case above, but Force should
        // show one anyway, clamped to whatever room is actually there.
        let line = TimedLine::new("hi".into(), 0.0, 4.0 + MIN_SING_DURATION, Singer::Male);
        let cd = countdown_window(&line, CountdownMode::Force, &TimingSettings::default());
        assert!(cd.is_some());
        let (start, end) = cd.unwrap();
        assert_eq!(end, line.end);
        assert!(start >= line.sing_end);
    }

    #[test]
    fn countdown_mode_force_shows_nothing_for_a_zero_or_negative_gap() {
        // No room at all before the next line starts - Force can't invent
        // one, so it should still show nothing rather than a degenerate
        // (zero- or negative-width) window.
        let line = TimedLine::new("hi there".into(), 0.0, 0.01, Singer::Male);
        assert!(line.sing_end >= line.end); // no leftover gap in this window
        assert!(
            countdown_window(&line, CountdownMode::Force, &TimingSettings::default()).is_none()
        );
    }

    #[test]
    fn countdown_mode_suppress_overrides_a_long_automatic_gap() {
        // Same long gap that already triggers Auto above - Suppress should
        // override it back off.
        let long_gap = TimedLine::new("hi".into(), 0.0, 20.0, Singer::Male);
        assert!(countdown_window(
            &long_gap,
            CountdownMode::Suppress,
            &TimingSettings::default()
        )
        .is_none());
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
        let settings = TimingSettings::default();
        let auto = CountdownMode::Auto;
        assert!(!hide_upcoming_lines(&long_gap, 0.0, auto, &settings)); // still singing
        assert!(!hide_upcoming_lines(
            &long_gap,
            long_gap.sing_end - 0.01,
            auto,
            &settings
        )); // just before done
        assert!(hide_upcoming_lines(
            &long_gap,
            long_gap.sing_end,
            auto,
            &settings
        )); // done, break starts
        assert!(hide_upcoming_lines(&long_gap, 19.9, auto, &settings)); // still in the break

        // Tight line, no meaningful gap -> never hide, even once "sung".
        let tight = TimedLine::new("hi there".into(), 0.0, 2.0, Singer::Male);
        assert!(!hide_upcoming_lines(
            &tight,
            tight.sing_end,
            auto,
            &settings
        ));
        assert!(!hide_upcoming_lines(&tight, 2.0, auto, &settings));
    }

    #[test]
    fn blank_sung_lines_waits_for_linger_then_clears_until_countdown() {
        // 30s gap: sing_end is at 2.0 (2 short words), so cd_start is at
        // end-4.0. Linger keeps the sung line up for SUNG_LINGER_SECS past
        // sing_end, then it should blank until the countdown begins.
        let line = TimedLine::new("hi there".into(), 0.0, 30.0, Singer::Male);
        let settings = TimingSettings::default();
        let auto = CountdownMode::Auto;
        let cd_start = countdown_window(&line, auto, &settings).unwrap().0;
        assert!(!blank_sung_lines(&line, line.sing_end, auto, &settings)); // just finished, still lingering
        assert!(!blank_sung_lines(
            &line,
            line.sing_end + SUNG_LINGER_SECS - 0.01,
            auto,
            &settings
        ));
        assert!(blank_sung_lines(
            &line,
            line.sing_end + SUNG_LINGER_SECS,
            auto,
            &settings
        )); // linger's up
        assert!(blank_sung_lines(&line, cd_start - 0.01, auto, &settings)); // still blank right up to the dots
        assert!(blank_sung_lines(&line, cd_start, auto, &settings)); // dots are on; sung line stays cleared

        // Short/no gap -> never blank.
        let tight = TimedLine::new("hi there".into(), 0.0, 2.0, Singer::Male);
        assert!(!blank_sung_lines(&tight, tight.sing_end, auto, &settings));
        assert!(!blank_sung_lines(&tight, 2.0, auto, &settings));
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

    // --- backing vocals ----------------------------------------------------

    #[test]
    fn backing_vocal_is_absent_until_its_start_is_tapped() {
        let mut line = LyricLine::new("main line here");
        line.start = Some(0.0);
        line.backing_vocal = Some(BackingVocal::new("echo"));
        let timed = TimedLine::from_line(&line, 0.0, 10.0);
        assert!(
            timed.backing_vocal.is_none(),
            "no start tapped yet -> nothing to render"
        );
    }

    #[test]
    fn backing_vocal_start_is_clamped_within_the_host_window() {
        let mut line = LyricLine::new("main line here");
        line.start = Some(0.0);
        let mut bv = BackingVocal::new("echo");
        bv.start = Some(-5.0); // before the host even starts
        bv.end = Some(999.0); // way past the host's own end
        line.backing_vocal = Some(bv);
        let timed = TimedLine::from_line(&line, 0.0, 10.0);
        let tbv = timed.backing_vocal.unwrap();
        assert_eq!(tbv.start, 0.0);
        assert_eq!(tbv.end, 10.0);
    }

    #[test]
    fn backing_vocal_end_defaults_to_the_hosts_own_end_when_untapped() {
        let mut line = LyricLine::new("main line here");
        line.start = Some(0.0);
        let mut bv = BackingVocal::new("echo");
        bv.start = Some(7.0);
        // bv.end left None - not tapped yet.
        line.backing_vocal = Some(bv);
        let timed = TimedLine::from_line(&line, 0.0, 10.0);
        let tbv = timed.backing_vocal.unwrap();
        assert_eq!(tbv.start, 7.0);
        assert_eq!(tbv.end, 10.0, "should default to the host's own end");
    }

    #[test]
    fn check_backing_vocal_start_must_fall_within_the_host_window() {
        assert!(check_backing_vocal_start(5.0, 0.0, 10.0).is_ok());
        assert!(check_backing_vocal_start(-1.0, 0.0, 10.0).is_err());
        assert!(check_backing_vocal_start(10.0, 0.0, 10.0).is_err()); // not < host_end
        assert!(check_backing_vocal_start(0.0, 0.0, 10.0).is_ok()); // exactly at host_start is fine
    }

    #[test]
    fn check_backing_vocal_end_must_come_after_its_own_start_and_not_past_the_host() {
        assert!(check_backing_vocal_end(8.0, 5.0, 10.0).is_ok());
        assert!(check_backing_vocal_end(5.0, 5.0, 10.0).is_err()); // not strictly after its own start
        assert!(check_backing_vocal_end(10.0, 5.0, 10.0).is_ok()); // exactly at host_end is fine
        assert!(check_backing_vocal_end(10.1, 5.0, 10.0).is_err()); // past the host
    }

    #[test]
    fn backing_vocal_word_timings_work_the_same_way_as_the_host_lines() {
        let mut line = LyricLine::new("main line");
        line.start = Some(0.0);
        let mut bv = BackingVocal::new("echo now");
        bv.start = Some(2.0);
        bv.end = Some(4.0);
        line.backing_vocal = Some(bv);
        let timed = TimedLine::from_line(&line, 0.0, 10.0);
        let tbv = timed.backing_vocal.unwrap();
        let words = backing_vocal_word_timings(&tbv);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].highlight_at, 2.0);
        assert!(words[1].highlight_at >= words[0].highlight_at);
        assert!(words[1].highlight_at < 4.0 + 1e-9);
    }

    #[test]
    fn a_reparse_carries_the_backing_vocal_through_for_an_unchanged_host_line() {
        let raw = "main line here";
        let mut lines = parse_pasted_lyrics(raw);
        lines[0].start = Some(0.0);
        let mut bv = BackingVocal::new("echo");
        bv.start = Some(1.0);
        bv.end = Some(2.0);
        lines[0].backing_vocal = Some(bv.clone());

        let (merged, report) = merge_reparsed_lyrics(&lines, raw);
        assert_eq!(report.unchanged, 1);
        assert_eq!(merged[0].backing_vocal, Some(bv));
    }

    #[test]
    fn a_reparse_carries_the_backing_vocal_through_for_a_reworded_host_line() {
        let old_raw = "hello world";
        let mut lines = parse_pasted_lyrics(old_raw);
        lines[0].start = Some(0.0);
        let mut bv = BackingVocal::new("echo");
        bv.start = Some(1.0);
        bv.end = Some(2.0);
        lines[0].backing_vocal = Some(bv.clone());

        // Same word count, one word changed - a "reworded" match, not an
        // exact one, per merge_edited_line - the backing vocal must still
        // survive, since it's attached to the host line's identity, not to
        // its exact wording.
        let (merged, report) = merge_reparsed_lyrics(&lines, "hello there");
        assert_eq!(report.reworded_same_word_count, 1);
        assert_eq!(merged[0].backing_vocal, Some(bv));
    }

    // --- merge_reparsed_lyrics -------------------------------------------

    /// Builds a timed `LyricLine` for merge tests - a start, a sing-end
    /// override, and every word manually overridden to a distinct time, so
    /// a test can tell at a glance whether a given field survived a merge.
    /// Only meaningful in isolation (`starts_new_block` always comes out
    /// `true`, as if it were the first/only line) - see
    /// [`timed_lines_from`] for a multi-line fixture with a realistic
    /// `starts_new_block` per line.
    fn timed_line(text: &str, start: f64) -> LyricLine {
        let mut line = LyricLine::new(text);
        line.start = Some(start);
        line.sing_end_override = Some(start + 10.0);
        line.singer = Singer::Duet;
        let n = line.word_overrides.len();
        for i in 0..n {
            line.word_overrides[i] = Some(start + i as f64);
            line.word_end_overrides[i] = Some(start + i as f64 + 0.5);
        }
        line
    }

    /// Builds a realistic "already timed" fixture: parses `raw` for real
    /// (so `starts_new_block` is whatever a genuine previous parse would
    /// have produced for this exact layout, not just a hardcoded default),
    /// then stamps each resulting line with distinct timing per `starts`.
    fn timed_lines_from(raw: &str, starts: &[f64]) -> Vec<LyricLine> {
        let mut lines = parse_pasted_lyrics(raw);
        assert_eq!(lines.len(), starts.len(), "fixture/starts length mismatch");
        for (line, &start) in lines.iter_mut().zip(starts) {
            line.start = Some(start);
            line.sing_end_override = Some(start + 10.0);
            line.singer = Singer::Duet;
            let n = line.word_overrides.len();
            for i in 0..n {
                line.word_overrides[i] = Some(start + i as f64);
                line.word_end_overrides[i] = Some(start + i as f64 + 0.5);
            }
        }
        lines
    }

    /// Compares every field *except* `starts_new_block`, which is always
    /// recomputed fresh from the new text's own block structure rather than
    /// carried over (see `starts_new_block_is_always_recomputed_fresh_not_carried_over`),
    /// so it's not part of what "timing was preserved" means and is
    /// legitimately allowed to differ when a line's position relative to
    /// blank lines changes (e.g. a line inserted before it).
    fn assert_timing_preserved(actual: &LyricLine, expected: &LyricLine) {
        assert_eq!(actual.text, expected.text);
        assert_eq!(actual.start, expected.start);
        assert_eq!(actual.singer, expected.singer);
        assert_eq!(actual.word_overrides, expected.word_overrides);
        assert_eq!(actual.word_end_overrides, expected.word_end_overrides);
        assert_eq!(actual.sing_end_override, expected.sing_end_override);
    }

    #[test]
    fn no_text_change_keeps_every_line_and_every_field_untouched() {
        let raw = "hello world\nsecond line";
        let old = timed_lines_from(raw, &[1.0, 5.0]);
        let (merged, report) = merge_reparsed_lyrics(&old, raw);

        assert_eq!(merged, old, "nothing about the lines should have changed");
        assert_eq!(report.unchanged, 2);
        assert!(report.is_fully_unchanged(2));
        assert_eq!(
            report.reworded_same_word_count
                + report.reworded_word_count_changed
                + report.added
                + report.removed,
            0
        );
    }

    #[test]
    fn editing_one_word_in_one_line_only_affects_that_lines_word_timing() {
        // "hello world" -> "hello there": same word count, one word changed.
        let old = timed_lines_from("hello world\nsecond line", &[1.0, 5.0]);
        let raw = "hello there\nsecond line";
        let (merged, report) = merge_reparsed_lyrics(&old, raw);

        assert_eq!(merged.len(), 2);
        // Untouched line: byte-for-byte identical, including word timing.
        assert_eq!(merged[1], old[1]);

        // Edited line: line-level timing kept...
        assert_eq!(merged[0].text, "hello there");
        assert_eq!(merged[0].start, old[0].start);
        assert_eq!(merged[0].sing_end_override, old[0].sing_end_override);
        assert_eq!(merged[0].singer, old[0].singer);
        // ...word 0 ("hello") unchanged -> keeps its override...
        assert_eq!(merged[0].word_overrides[0], old[0].word_overrides[0]);
        assert_eq!(
            merged[0].word_end_overrides[0],
            old[0].word_end_overrides[0]
        );
        // ...word 1 ("world" -> "there") changed -> resets to automatic.
        assert_eq!(merged[0].word_overrides[1], None);
        assert_eq!(merged[0].word_end_overrides[1], None);

        assert_eq!(report.unchanged, 1);
        assert_eq!(report.reworded_same_word_count, 1);
        assert_eq!(report.reworded_word_count_changed, 0);
        assert_eq!(report.added, 0);
        assert_eq!(report.removed, 0);
    }

    #[test]
    fn changing_a_lines_word_count_keeps_line_timing_but_resets_word_timing() {
        let old = vec![timed_line("hello world", 1.0)];
        let raw = "hello there world"; // 2 words -> 3 words
        let (merged, report) = merge_reparsed_lyrics(&old, raw);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "hello there world");
        assert_eq!(merged[0].start, old[0].start);
        assert_eq!(merged[0].sing_end_override, old[0].sing_end_override);
        assert_eq!(merged[0].singer, old[0].singer);
        assert_eq!(merged[0].word_overrides, vec![None, None, None]);
        assert_eq!(merged[0].word_end_overrides, vec![None, None, None]);

        assert_eq!(report.reworded_word_count_changed, 1);
        assert_eq!(report.reworded_same_word_count, 0);
        assert_eq!(report.unchanged, 0);
    }

    #[test]
    fn inserting_a_new_line_in_the_middle_keeps_every_existing_lines_timing() {
        let old = timed_lines_from("first\nsecond\nthird", &[1.0, 5.0, 9.0]);
        let raw = "first\nbrand new line\nsecond\nthird";
        let (merged, report) = merge_reparsed_lyrics(&old, raw);

        assert_eq!(merged.len(), 4);
        assert_eq!(merged[0], old[0]); // "first" untouched
        assert_eq!(merged[1].text, "brand new line");
        assert_eq!(merged[1].start, None); // new line starts untimed
        assert_eq!(merged[2], old[1]); // "second" untouched, timing intact
        assert_eq!(merged[3], old[2]); // "third" untouched, timing intact

        assert_eq!(report.unchanged, 3);
        assert_eq!(report.added, 1);
        assert_eq!(report.removed, 0);
        assert_eq!(report.reworded_same_word_count, 0);
        assert_eq!(report.reworded_word_count_changed, 0);
    }

    #[test]
    fn deleting_a_line_only_loses_that_lines_timing() {
        let old = timed_lines_from("first\nsecond\nthird", &[1.0, 5.0, 9.0]);
        let raw = "first\nthird"; // "second" deleted
        let (merged, report) = merge_reparsed_lyrics(&old, raw);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0], old[0]);
        assert_eq!(merged[1], old[2]);

        assert_eq!(report.unchanged, 2);
        assert_eq!(report.removed, 1);
        assert_eq!(report.added, 0);
    }

    #[test]
    fn a_duplicated_line_keeps_each_occurrences_own_distinct_timing() {
        // Two identical "chorus" lines with *different* timing - re-parsing
        // unchanged text must not swap them.
        let raw = "we will rock you\nverse between\nwe will rock you";
        let mut old = timed_lines_from(raw, &[10.0, 30.0, 50.0]);
        old[0].singer = Singer::Male;
        old[2].singer = Singer::Female;
        let (chorus1, chorus2) = (old[0].clone(), old[2].clone());
        let (merged, report) = merge_reparsed_lyrics(&old, raw);

        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0], chorus1, "first chorus keeps its own timing");
        assert_eq!(merged[1], old[1]);
        assert_eq!(merged[2], chorus2, "second chorus keeps its own timing");
        assert_ne!(
            merged[0].start, merged[2].start,
            "the two occurrences must not have been swapped/merged together"
        );
        assert_eq!(report.unchanged, 3);
    }

    #[test]
    fn a_moved_line_keeps_its_timing_by_content_not_by_position() {
        // Simulates pasting a new line *above* an already-timed block - the
        // existing lines shift down by one index but their text (and
        // therefore their timing) doesn't change.
        let old = timed_lines_from("verse one\nverse two", &[10.0, 15.0]);
        let raw = "brand new intro line\nverse one\nverse two";
        let (merged, _report) = merge_reparsed_lyrics(&old, raw);

        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].text, "brand new intro line");
        assert_eq!(merged[0].start, None);
        // Despite now sitting at index 1/2 instead of 0/1, both keep their
        // original absolute timing - it's still correct, since the song
        // itself didn't change, only where this text sits in the list.
        // `starts_new_block` legitimately differs for "verse one" here (it
        // was first-in-block in `old`, but no longer is, now that a new
        // line precedes it) - see `assert_timing_preserved`.
        assert_timing_preserved(&merged[1], &old[0]);
        assert_timing_preserved(&merged[2], &old[1]);
    }

    #[test]
    fn starts_new_block_is_always_recomputed_fresh_not_carried_over() {
        // Old data claims "second" doesn't start a new block; the new text
        // has a blank line before it, which must win regardless of what the
        // old (now-stale) flag said.
        let mut old = vec![timed_line("first", 1.0), timed_line("second", 5.0)];
        old[1].starts_new_block = false;
        let raw = "first\n\nsecond"; // blank line inserted before "second"
        let (merged, _report) = merge_reparsed_lyrics(&old, raw);

        assert!(merged[1].starts_new_block);
        // Timing itself is still preserved even though this flag wasn't.
        assert_eq!(merged[1].start, old[1].start);
    }

    #[test]
    fn empty_old_lines_behaves_like_a_plain_first_parse() {
        let old: Vec<LyricLine> = Vec::new();
        let raw = "a\nb\nc";
        let (merged, report) = merge_reparsed_lyrics(&old, raw);
        assert_eq!(merged, parse_pasted_lyrics(raw));
        assert_eq!(report.added, 3);
        assert_eq!(report.unchanged, 0);
    }

    #[test]
    fn clearing_the_text_entirely_drops_every_line_and_its_timing() {
        let old = vec![timed_line("first", 1.0), timed_line("second", 5.0)];
        let (merged, report) = merge_reparsed_lyrics(&old, "");
        assert!(merged.is_empty());
        assert_eq!(report.removed, 2);
    }

    #[test]
    fn lcs_matches_is_a_strictly_increasing_valid_alignment() {
        let old = parse_pasted_lyrics("a\nb\na\nc\nb");
        let new = parse_pasted_lyrics("x\na\nb\na\ny\nb");
        let pairs = lcs_matches(&old, &new);
        // Every matched pair must actually match by text...
        for &(i, j) in &pairs {
            assert_eq!(old[i].text, new[j].text);
        }
        // ...and indices must be strictly increasing in both lists (a valid
        // common-subsequence alignment, not just any pairing).
        for w in pairs.windows(2) {
            assert!(w[0].0 < w[1].0);
            assert!(w[0].1 < w[1].1);
        }
        // "a b a c b" vs "x a b a y b" shares "a b a b" as a common
        // subsequence (length 4) - confirm the LCS actually finds it, not
        // some shorter alignment.
        assert_eq!(pairs.len(), 4);
    }

    #[test]
    fn summarize_reports_a_pure_no_op_distinctly() {
        let report = ReparseReport {
            unchanged: 3,
            ..Default::default()
        };
        assert!(report.summarize(3).contains("no text changed"));
    }
}
