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
    /// A dedicated color for backing vocals - what a line switched to the
    /// backing track (see [`LyricLine::is_backing`]) gets by default, so a
    /// second vocalist singing different lyrics over the lead stands apart
    /// from every lead voice. Usable on any line like the others, though.
    Backing,
}

impl Singer {
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Male => "Male",
            Self::Female => "Female",
            Self::Duet => "Duet",
            Self::Screaming => "Screaming",
            Self::Backing => "Backing",
        }
    }

    /// Every variant, in a fixed display order - used both to build the
    /// color legend (see [`singer_legend`]) and, in `main.rs`, to build the
    /// timing table's bulk "set voice for selection" buttons from the same
    /// single list the per-line dropdown's options come from.
    pub const ALL: [Singer; 6] = [
        Self::Default,
        Self::Male,
        Self::Female,
        Self::Duet,
        Self::Screaming,
        Self::Backing,
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
/// distinct *label* in play, since a legend only earns its place once more
/// than one is actually shown - e.g. a duet, a song with a dedicated
/// screaming section, or two custom names sharing one base color (see
/// [`effective_singer_label`]) - which also means an untouched song (every
/// line still at `Default`, no custom names) never gets a legend. Each
/// entry pairs a line's `singer` (which color to use) with whatever custom
/// name (if any) was typed for it - two lines with the same `singer` but
/// different custom names are two separate entries, not merged into one.
///
/// `backing` is the backing track (see [`resolve_backing_track`]) - its
/// voices count towards the legend exactly like the main track's do.
pub fn singer_legend(lines: &[TimedLine], backing: &[TimedLine]) -> Vec<(Singer, Option<String>)> {
    let mut used: Vec<(Singer, Option<String>)> = Vec::new();
    for base in Singer::ALL {
        for line in lines.iter().chain(backing) {
            if line.singer != base {
                continue;
            }
            let key = (line.singer, line.custom_singer_name.clone());
            if !used.contains(&key) {
                used.push(key);
            }
        }
    }
    if used.len() > 1 {
        used
    } else {
        Vec::new()
    }
}

/// The text shown for a singer in a legend/dropdown - `custom_name` if
/// there is one, otherwise `singer`'s own generic label (see
/// [`Singer::label`]). `custom_name` never changes what color `singer`
/// renders in - only what's shown as text (see
/// [`LyricLine::custom_singer_name`]'s own docs for why: this app's `.cdg`
/// exporter has a hard 16-color palette with no room for arbitrarily many
/// distinct singer colors).
pub fn effective_singer_label(singer: Singer, custom_name: &Option<String>) -> &str {
    custom_name.as_deref().unwrap_or_else(|| singer.label())
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
/// Used directly by the `.cdg` exporter's own packet-scheduling code
/// (`export.rs`'s `render_cdg`, computing when to emit its clear
/// instruction) for its hard cutoff - the video export and live preview
/// use the separate, per-line fade timing below instead (`.cdg`'s fixed
/// 16-color palette isn't a reasonable place to do a smooth per-line fade
/// the way full RGB output can).
pub const SUNG_LINGER_SECS: f64 = 5.0;

/// How long an already-sung line stays fully visible, in the video export
/// and live preview specifically, before it starts fading out - see
/// [`sung_line_fade_alpha`]. Deliberately a separate constant from
/// [`SUNG_LINGER_SECS`] (not a shared one), even though they mean similar
/// things: the fade itself is what makes a shorter linger read as smooth
/// rather than abrupt, so the two aren't interchangeable, and `.cdg`'s own
/// hard-cutoff timing shouldn't change just because the fade-capable
/// formats' timing does.
pub const SUNG_FADE_LINGER_SECS: f64 = 2.5;
/// How long the fade-out itself takes, once it starts.
pub const SUNG_FADE_DURATION_SECS: f64 = 1.75;
/// How long a freshly revealed line/block takes to fade *in* - the
/// countdown's own upcoming-line reveal, and a new block's first
/// appearance. Shorter than the fade-out so new content doesn't feel slow
/// to arrive.
pub const REVEAL_FADE_DURATION_SECS: f64 = 1.0;

/// Opacity (0.0-1.0) for an already-sung line at time `t`, given when it
/// finished being sung (`sing_end`) - full opacity for
/// [`SUNG_FADE_LINGER_SECS`], then a linear fade to 0 over
/// [`SUNG_FADE_DURATION_SECS`], then 0 after that. Video export/live
/// preview only - see [`SUNG_FADE_LINGER_SECS`]'s own docs for why `.cdg`
/// doesn't use this.
pub fn sung_line_fade_alpha(sing_end: f64, t: f64) -> f32 {
    let since = t - sing_end;
    if since <= SUNG_FADE_LINGER_SECS {
        1.0
    } else {
        let frac = (since - SUNG_FADE_LINGER_SECS) / SUNG_FADE_DURATION_SECS.max(0.001);
        (1.0 - frac).clamp(0.0, 1.0) as f32
    }
}

/// Opacity (0.0-1.0) for a line/block fading *in* once revealed at
/// `reveal_at` (e.g. a countdown's own start time, or the moment a fresh
/// block first becomes current) - 0 right at `reveal_at`, ramping linearly
/// to full opacity over [`REVEAL_FADE_DURATION_SECS`]. Negative (i.e.
/// `t < reveal_at`) clamps to 0, same as "not revealed yet".
pub fn reveal_fade_alpha(reveal_at: f64, t: f64) -> f32 {
    let since = t - reveal_at;
    (since / REVEAL_FADE_DURATION_SECS.max(0.001)).clamp(0.0, 1.0) as f32
}

/// Opacity for a block slot showing `line` at time `t`: full while it's
/// still the actively-singing current line or hasn't started yet (`t <
/// line.sing_end`), otherwise the same fade-out curve as
/// [`sung_line_fade_alpha`] once it's done. Slot opacity is monotonically
/// non-increasing from the top of a block down (each line's `sing_end` is
/// later than the one before it, so an earlier slot has always had at
/// least as long to fade as a later one) - [`rolling_preview_lines`] relies
/// on that to know a block's fully-faded slots are always a clean prefix,
/// never a gap in the middle.
pub fn slot_fade_alpha(line: &TimedLine, t: f64) -> f32 {
    if t < line.sing_end {
        1.0
    } else {
        sung_line_fade_alpha(line.sing_end, t)
    }
}

/// For a block whose top `freed_slots` rows have fully faded out (opacity
/// 0 via [`slot_fade_alpha`]), returns up to `freed_slots` line indices
/// from *beyond* this block (in singing order) to preview in those now-
/// empty rows - a rolling, KaraFun-style hand-off to whatever comes next
/// when the gap is short enough that a countdown wouldn't trigger for it
/// anyway. Stops (returning fewer than `freed_slots`) the moment a real
/// countdown-worthy gap is hit - see [`countdown_window`] - or the song
/// runs out of lines; that transition is left to the existing countdown/
/// reveal mechanism instead, not blended with this one. `block` is the
/// current on-screen block's own line indices, in order.
pub fn rolling_preview_lines(
    timed_lines: &[TimedLine],
    block: &[usize],
    freed_slots: usize,
    settings: &TimingSettings,
) -> Vec<usize> {
    let mut result = Vec::new();
    let Some(&last_idx) = block.last() else {
        return result;
    };
    let mut prev = &timed_lines[last_idx];
    let mut next_idx = last_idx + 1;
    while result.len() < freed_slots {
        let Some(candidate) = timed_lines.get(next_idx) else {
            break;
        };
        if countdown_window(prev, candidate.countdown_mode, settings).is_some() {
            break;
        }
        result.push(next_idx);
        prev = candidate;
        next_idx += 1;
    }
    result
}

fn estimate_sing_duration(text: &str, window: f64, settings: &TimingSettings) -> f64 {
    // Whole words, not syllables - marking `beau|ti|ful` doesn't make the
    // line take any longer to sing.
    let word_count = text.split_whitespace().count().max(1);
    let est = (word_count as f64 * settings.seconds_per_word).max(settings.min_sing_duration);
    est.min(window.max(0.0))
}

/// The project's outro card settings: a closing card shown after the last
/// line until the song ends (see `export::outro_window`), in the video,
/// live preview, `.cdg` and subtitle exports. Off by default, so existing
/// projects don't change.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct OutroSettings {
    pub enabled: bool,
    /// The card's own text - one line per line, up to [`MAX_OUTRO_LINES`].
    pub text: String,
    /// Also show the song title and "by artist" under the text.
    pub show_title: bool,
}

/// How many lines of [`OutroSettings::text`] are shown (the `.cdg` has
/// room for only a few rows).
pub const MAX_OUTRO_LINES: usize = 3;

impl Default for OutroSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            text: "Thanks for singing!".to_string(),
            show_title: true,
        }
    }
}

impl OutroSettings {
    /// The non-blank lines of the text, trimmed, at most [`MAX_OUTRO_LINES`].
    pub fn text_lines(&self) -> Vec<String> {
        self.text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .take(MAX_OUTRO_LINES)
            .map(str::to_string)
            .collect()
    }
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
    /// A lyric-file importer's best guess at how long this line is sung,
    /// in seconds measured from `start` - used instead of the plain
    /// word-count estimate (see [`estimate_sing_duration`]) when there's no
    /// [`LyricLine::sing_end_override`], so it only ever shows up as the
    /// End column's "auto" hint and in auto-align's search window, never
    /// as a manually-set end. Relative to `start` (not absolute) so it
    /// moves along with the line if its start is re-tapped or dragged.
    /// See `formats.rs`'s `estimate_imported_sing_duration`. `None` (via
    /// `#[serde(default)]`) for typed/pasted lines and every project saved
    /// before this existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_sing_duration: Option<f64>,
    /// True if this line should start a new on-screen verse block in the
    /// video export - i.e. it was preceded by a blank line in the pasted
    /// lyrics (or it's the very first line). Defaults to `true` for lines
    /// created outside of [`parse_pasted_lyrics`] (e.g. in tests), since
    /// treating every line as its own block is a safer default than
    /// silently merging unrelated lines together.
    pub starts_new_block: bool,
    /// True if this line belongs to the *backing track* rather than the
    /// main one - a second vocalist singing different lyrics over the lead
    /// (a duet where both singers overlap, a screamed backing part, ...).
    /// A backing line is still an ordinary entry in the pasted lyrics and
    /// in this list (so re-parsing keeps it in place like any other line),
    /// but its timing is fully independent: it's left out of the main
    /// track's sequential, non-overlapping [`resolve_timing_with_settings`]
    /// and resolved separately by [`resolve_backing_track`] instead, so it
    /// can start/end anywhere, overlapping any main line(s). Its end is
    /// [`Self::sing_end_override`] (or an estimate until that's tapped).
    /// `false` for every line saved before this existed, via
    /// `#[serde(default)]`.
    #[serde(default)]
    pub is_backing: bool,
    /// **Legacy, load-only.** The old "Echo" feature stored a backing vocal
    /// embedded on its host line, bounded within the host's own window.
    /// That's been replaced by [`Self::is_backing`] lines;
    /// [`migrate_legacy_backing_vocals`] converts any of these found in an
    /// older project into backing lines right after loading, so this is
    /// always `None` afterwards (and is never written back out).
    #[serde(default, skip_serializing)]
    pub backing_vocal: Option<BackingVocal>,
    /// Manual override for whether the "get ready" countdown shows up
    /// before *this* line - see [`CountdownMode`]. `Auto` (the default,
    /// via `#[serde(default)]`) for every line/project saved before this
    /// existed, matching the automatic-only behavior they already had.
    #[serde(default)]
    pub countdown_mode: CountdownMode,
    /// A user-typed display name ("Male 1", "Lead Singer", ...) shown in
    /// the legend/dropdown instead of `singer`'s own generic label -
    /// `singer` still decides this line's *color* (there's no separate
    /// color budget for custom names - see [`effective_singer_label`]),
    /// this only overrides what's shown as text. `None` (the default, via
    /// `#[serde(default)]`) for every line/project saved before this
    /// existed, and for any line that hasn't been given a custom name.
    #[serde(default)]
    pub custom_singer_name: Option<String>,
}

impl LyricLine {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let word_count = timing_units(&text).len();
        Self {
            text,
            start: None,
            singer: Singer::default(),
            word_overrides: vec![None; word_count],
            word_end_overrides: vec![None; word_count],
            sing_end_override: None,
            estimated_sing_duration: None,
            starts_new_block: true,
            is_backing: false,
            backing_vocal: None,
            countdown_mode: CountdownMode::default(),
            custom_singer_name: None,
        }
    }

    /// The line's timing units - words, split into syllables wherever
    /// the text has a [`SYLLABLE_MARK`] (see [`timing_units`]). Per-word
    /// timing (`word_overrides`, ...) is indexed by these.
    pub fn words(&self) -> Vec<&str> {
        timing_units(&self.text)
            .into_iter()
            .map(|(u, _)| u)
            .collect()
    }
}

/// **Legacy** - the old "Echo" backing vocal, embedded on its host
/// [`LyricLine`] and bounded within the host's own window. Only still here
/// so projects saved with one keep loading; see
/// [`migrate_legacy_backing_vocals`], which turns each into an independent
/// [`LyricLine::is_backing`] line.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BackingVocal {
    pub text: String,
    /// Start time in seconds (was bounded within the host line's window).
    /// `None` means "not yet tapped".
    pub start: Option<f64>,
    /// End time in seconds. `None` meant "use the host line's own end".
    pub end: Option<f64>,
    pub singer: Singer,
    /// Same shape/meaning as [`LyricLine::word_overrides`], scoped to this
    /// backing vocal's own words.
    pub word_overrides: Vec<Option<f64>>,
    /// Same shape/meaning as [`LyricLine::word_end_overrides`].
    pub word_end_overrides: Vec<Option<f64>>,
}

impl BackingVocal {
    #[allow(dead_code)]
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let word_count = timing_units(&text).len();
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
    parse_pasted_lyrics_with_sources(raw)
        .into_iter()
        .map(|(line, _)| line)
        .collect()
}

/// Same as [`parse_pasted_lyrics`], but also returns, for each parsed line,
/// the index of the `raw` text line it came from (blank lines and section
/// markers count towards that index, since they're still lines of `raw`).
fn parse_pasted_lyrics_with_sources(raw: &str) -> Vec<(LyricLine, usize)> {
    let mut result: Vec<(LyricLine, usize)> = Vec::new();
    let mut pending_blank = false;
    for (raw_idx, raw_line) in raw.lines().enumerate() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || is_section_marker(trimmed) {
            pending_blank = true;
            continue;
        }
        let mut line = LyricLine::new(trimmed);
        line.starts_new_block = result.is_empty() || pending_blank;
        result.push((line, raw_idx));
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
/// When more than one alignment matches the same number of lines - e.g. a
/// repeated line pasted in again right before an identical copy of itself,
/// where text alone can't say which copy is the new one - `edited` breaks
/// the tie: new lines flagged `true` (the region of the text box the user
/// actually typed/pasted into) are left unmatched in preference to
/// unflagged ones, so the untimed line lands where the user put it rather
/// than on the other copy. `edited` may be shorter than `new` (missing
/// entries count as `false`), so passing `&[]` means "no hint".
///
/// O(n*m) time and space - fine for realistic lyric line counts (tens to a
/// few hundred), not chosen with e.g. a full song's worth of KOK-imported
/// word-level fragments in mind.
fn lcs_matches(old: &[LyricLine], new: &[LyricLine], edited: &[bool]) -> Vec<(usize, usize)> {
    let n = old.len();
    let m = new.len();
    // Each match is worth `full`, or one less inside the edited region -
    // since at most `m` matches can each lose 1, the total penalty is always
    // less than one whole match, so the number of matched lines is still
    // maximized first and the hint only ever decides between equal-sized
    // alignments.
    let full = m as u64 + 1;
    let weight = |j: usize| {
        if edited.get(j).copied().unwrap_or(false) {
            full - 1
        } else {
            full
        }
    };
    let mut dp = vec![vec![0u64; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            let skip = dp[i + 1][j].max(dp[i][j + 1]);
            dp[i][j] = if old[i].text == new[j].text {
                skip.max(dp[i + 1][j + 1] + weight(j))
            } else {
                skip
            };
        }
    }

    let mut pairs = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if old[i].text == new[j].text && dp[i][j] == dp[i + 1][j + 1] + weight(j) {
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
/// `estimated_sing_duration`) and per-line settings (`singer`,
/// `custom_singer_name`, `countdown_mode`, `is_backing`) are always kept -
/// a typo fix or reworded phrase doesn't change *when* the line is sung or
/// who sings it. Word-level timing is only kept where it can
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
    merged.estimated_sing_duration = old.estimated_sing_duration;
    merged.singer = old.singer;
    // Main-vs-backing track is part of the line's identity, not its exact
    // text - a reworded backing line (typo fix, rephrasing) stays on the
    // backing track, same as it keeps its own timing.
    merged.is_backing = old.is_backing;
    // Same for the line's voice label and countdown choice - both are
    // per-line settings the user made, not something derived from the text.
    merged.custom_singer_name = old.custom_singer_name.clone();
    merged.countdown_mode = old.countdown_mode;

    let old_words: Vec<&str> = old.text.split_whitespace().collect();
    let new_words: Vec<&str> = new.text.split_whitespace().collect();
    let same_word_count = old_words.len() == new_words.len();
    if same_word_count {
        // Per-word timing is indexed by timing unit (syllable), so walk
        // both lines word by word, tracking each word's first unit.
        let units_in = |w: &str| w.split(SYLLABLE_MARK).filter(|p| !p.is_empty()).count();
        let (mut oi, mut ni) = (0usize, 0usize);
        for (&ow, &nw) in old_words.iter().zip(new_words.iter()) {
            let (on, nn) = (units_in(ow), units_in(nw));
            let copy = |merged: &mut LyricLine, from: usize, to: usize| {
                if let Some(slot) = merged.word_overrides.get_mut(to) {
                    *slot = old.word_overrides.get(from).copied().flatten();
                }
            };
            let copy_end = |merged: &mut LyricLine, from: usize, to: usize| {
                if let Some(slot) = merged.word_end_overrides.get_mut(to) {
                    *slot = old.word_end_overrides.get(from).copied().flatten();
                }
            };
            if ow == nw {
                for k in 0..nn {
                    copy(&mut merged, oi + k, ni + k);
                    copy_end(&mut merged, oi + k, ni + k);
                }
            } else if on > 0
                && nn > 0
                && ow.replace(SYLLABLE_MARK, "") == nw.replace(SYLLABLE_MARK, "")
            {
                // Same word, only its syllable marks changed: its own
                // start/end still hold - they now belong to its first/last
                // syllable, and the syllables in between start out estimated.
                copy(&mut merged, oi, ni);
                copy_end(&mut merged, oi + on - 1, ni + nn - 1);
            }
            // A word that itself changed keeps `merged`'s fresh `None` from
            // `new.clone()` above - carrying over a manually-tapped time for
            // a word that no longer exists would silently mistime whatever
            // word replaced it.
            oi += on;
            ni += nn;
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
///
/// `edited_raw_lines`, if known, is the range of `raw`'s text lines the
/// user actually changed since the last parse - only used to break ties
/// between identical lines (see [`lcs_matches`]), so `None` is always safe.
pub fn merge_reparsed_lyrics(
    old: &[LyricLine],
    raw: &str,
    edited_raw_lines: Option<std::ops::Range<usize>>,
) -> (Vec<LyricLine>, ReparseReport) {
    let (new, edited): (Vec<LyricLine>, Vec<bool>) = parse_pasted_lyrics_with_sources(raw)
        .into_iter()
        .map(|(line, raw_idx)| {
            let edited = edited_raw_lines
                .as_ref()
                .is_some_and(|r| r.contains(&raw_idx));
            (line, edited)
        })
        .unzip();
    let anchors = lcs_matches(old, &new, &edited);

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
            kept.estimated_sing_duration = old[oi].estimated_sing_duration;
            kept.singer = old[oi].singer;
            kept.word_overrides = old[oi].word_overrides.clone();
            kept.word_end_overrides = old[oi].word_end_overrides.clone();
            kept.is_backing = old[oi].is_backing;
            kept.custom_singer_name = old[oi].custom_singer_name.clone();
            kept.countdown_mode = old[oi].countdown_mode;
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

/// The nearest main-track (non-[`LyricLine::is_backing`]) line before
/// `idx`, if any - a main line's neighbor rules skip over backing lines,
/// which are free to overlap it.
fn prev_main_line(lines: &[LyricLine], idx: usize) -> Option<&LyricLine> {
    lines[..idx].iter().rev().find(|l| !l.is_backing)
}

/// The nearest main-track line after `idx`, if any - see [`prev_main_line`].
fn next_main_line(lines: &[LyricLine], idx: usize) -> Option<&LyricLine> {
    lines.get(idx + 1..)?.iter().find(|l| !l.is_backing)
}

/// Checks whether setting `lines[idx]`'s `start` to `new_start` is valid -
/// non-negative, before this line's own explicit end (if any), and not
/// before the previous main-track line's explicit end (if any) - *without*
/// applying it. Callers (tapping, dragging, or typing a new start) should
/// only commit the change once this returns `Ok`, and show the message to
/// the user otherwise.
///
/// Deliberately only checks against an explicit [`LyricLine::sing_end_override`]
/// on either side, not the automatic estimate - the estimate shifts as
/// timing fills in around it, so treating it as a hard wall would reject
/// perfectly reasonable taps just because a neighboring line hasn't been
/// fine-tuned yet.
///
/// A backing line ([`LyricLine::is_backing`]) has no neighbor rule at all -
/// overlapping other lines is the whole point of one - and main lines skip
/// over backing lines when looking for their own neighbors.
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
    if lines[idx].is_backing {
        return Ok(());
    }
    if let Some(prev_end) = prev_main_line(lines, idx).and_then(|l| l.sing_end_override) {
        if new_start < prev_end {
            return Err(format!(
                "Start ({}) can't be before the previous line ends ({}).",
                format_timecode(new_start),
                format_timecode(prev_end)
            ));
        }
    }
    Ok(())
}

/// Checks whether setting `lines[idx]`'s [`LyricLine::sing_end_override`] to
/// `new_end` is valid - after this line's own start, and not after the
/// next main-track line's start (if that's already been set) - *without*
/// applying it. See [`check_start_change`] for why only explicit
/// neighboring times are checked, not automatic estimates, and why backing
/// lines are exempt from the neighbor rule.
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
    if lines[idx].is_backing {
        return Ok(());
    }
    if let Some(next_start) = next_main_line(lines, idx).and_then(|l| l.start) {
        if new_end > next_start {
            return Err(format!(
                "End ({}) can't be after the next line starts ({}).",
                format_timecode(new_end),
                format_timecode(next_start)
            ));
        }
    }
    Ok(())
}

/// Moves every line in `indices` by `delta` seconds - start, explicit end,
/// and every per-word start/end override together, so each line keeps its
/// own internal timing exactly. Lines with no start yet are left alone.
/// All-or-nothing: the shift is checked against the same rules as typing a
/// time (no negative times, main lines can't overlap their main-line
/// neighbors) on a copy first, and on any violation nothing changes and
/// the reason is returned. Returns how many lines actually moved.
pub fn shift_lines(
    lines: &mut [LyricLine],
    indices: &[usize],
    delta: f64,
) -> Result<usize, String> {
    if !delta.is_finite() {
        return Err("Shift amount must be a number.".to_string());
    }
    let mut shifted = lines.to_vec();
    let shift = |t: &mut Option<f64>| {
        if let Some(v) = t {
            *v += delta;
        }
    };
    let mut moved = 0;
    for &i in indices {
        let Some(line) = shifted.get_mut(i) else {
            continue;
        };
        if line.start.is_none() {
            continue;
        }
        shift(&mut line.start);
        shift(&mut line.sing_end_override);
        line.word_overrides.iter_mut().for_each(shift);
        line.word_end_overrides.iter_mut().for_each(shift);
        moved += 1;
    }
    if moved == 0 {
        return Err("None of the selected lines are timed yet - nothing to shift.".to_string());
    }
    for i in 0..shifted.len() {
        let line = &shifted[i];
        let earliest = line
            .start
            .into_iter()
            .chain(line.word_overrides.iter().flatten().copied())
            .chain(line.word_end_overrides.iter().flatten().copied())
            .fold(f64::INFINITY, f64::min);
        if earliest < 0.0 {
            return Err(format!(
                "Can't shift that far - line {} would start before the song does.",
                i + 1
            ));
        }
        if let Some(start) = line.start {
            check_start_change(&shifted, i, start).map_err(|e| format!("Line {}: {e}", i + 1))?;
        }
        if let Some(end) = shifted[i].sing_end_override {
            check_end_change(&shifted, i, end).map_err(|e| format!("Line {}: {e}", i + 1))?;
        }
    }
    lines.clone_from_slice(&shifted);
    Ok(moved)
}

/// A line's text reduced for "is this the same lyric?" comparisons:
/// displayed words, lowercase, punctuation dropped - so "Hey!" matches
/// "hey" and a repeat with different capitalization still counts.
pub fn comparable_text(text: &str) -> String {
    normalize_text(text)
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where the lines at `sources` (sorted, consecutive or not) next repeat
/// as a run of consecutive lines with the same text (see
/// [`comparable_text`]) - the first index of that run. Searches after the
/// last source line first, then from the top (a later chorus may be timed
/// first), never overlapping the sources.
pub fn find_repeat(lines: &[LyricLine], sources: &[usize]) -> Option<usize> {
    let n = sources.len();
    if n == 0 || n > lines.len() {
        return None;
    }
    let want: Vec<String> = sources
        .iter()
        .map(|&i| comparable_text(&lines[i].text))
        .collect();
    let last = *sources.iter().max()?;
    let first = *sources.iter().min()?;
    let matches_at = |j: usize| (0..n).all(|k| comparable_text(&lines[j + k].text) == want[k]);
    let overlaps = |j: usize| j <= last && j + n > first;
    (last + 1..=lines.len() - n)
        .chain(0..=lines.len() - n)
        .find(|&j| !overlaps(j) && matches_at(j))
}

/// Copies the timing of the lines at `sources` (sorted; each must be
/// timed) onto the `sources.len()` consecutive lines starting at
/// `first_target`, shifted so the first target starts at `target_start` -
/// for a repeated chorus/verse sung to the same rhythm. Each target gets
/// its source's start, explicit end and - where it has the same number of
/// timing units (words/syllables) - every word's start/end override, all
/// moved by the same offset; a target whose words differ gets its start
/// and end only (its word timing reset to the estimate). Voice, backing
/// status and countdown setting stay the target's own.
///
/// All-or-nothing: checked on a copy like a typed time (no overlaps with
/// neighboring lines, nothing before 0:00), and on any problem nothing
/// changes and the reason is returned. Returns how many target lines had
/// different words (so their word timing wasn't copied).
pub fn copy_timing(
    lines: &mut [LyricLine],
    sources: &[usize],
    first_target: usize,
    target_start: f64,
) -> Result<usize, String> {
    let n = sources.len();
    if n == 0 {
        return Err("Select the timed lines to copy from first.".to_string());
    }
    if first_target + n > lines.len() {
        return Err(format!(
            "There aren't {n} lines from line {} on to copy onto.",
            first_target + 1
        ));
    }
    let (lo, hi) = (
        *sources.iter().min().unwrap(),
        *sources.iter().max().unwrap(),
    );
    if first_target <= hi && first_target + n > lo {
        return Err("The lines to copy onto overlap the lines being copied.".to_string());
    }
    if !target_start.is_finite() || target_start < 0.0 {
        return Err("The repeat's start time can't be negative.".to_string());
    }
    let Some(base) = lines[sources[0]].start else {
        return Err(format!("Line {} isn't timed yet.", sources[0] + 1));
    };
    if let Some(&i) = sources.iter().find(|&&i| lines[i].start.is_none()) {
        return Err(format!(
            "Line {} isn't timed yet - every copied line must be.",
            i + 1
        ));
    }
    let offset = target_start - base;
    let shift = |t: Option<f64>| t.map(|v| v + offset);
    let mut out = lines.to_vec();
    let mut differing = 0;
    for (k, &src) in sources.iter().enumerate() {
        let from = &lines[src];
        let to = &mut out[first_target + k];
        to.start = shift(from.start);
        to.sing_end_override = shift(from.sing_end_override);
        to.estimated_sing_duration = from.estimated_sing_duration;
        let units = timing_units(&to.text).len();
        if timing_units(&from.text).len() == units {
            to.word_overrides = from.word_overrides.iter().map(|t| shift(*t)).collect();
            to.word_end_overrides = from.word_end_overrides.iter().map(|t| shift(*t)).collect();
            to.word_overrides.resize(units, None);
            to.word_end_overrides.resize(units, None);
        } else {
            to.word_overrides = vec![None; units];
            to.word_end_overrides = vec![None; units];
            differing += 1;
        }
    }
    for i in 0..out.len() {
        let earliest = out[i]
            .start
            .into_iter()
            .chain(out[i].word_overrides.iter().flatten().copied())
            .fold(f64::INFINITY, f64::min);
        if earliest < 0.0 {
            return Err(format!("Line {} would start before the song does.", i + 1));
        }
        if let Some(start) = out[i].start {
            check_start_change(&out, i, start).map_err(|e| format!("Line {}: {e}", i + 1))?;
        }
        if let Some(end) = out[i].sing_end_override {
            check_end_change(&out, i, end).map_err(|e| format!("Line {}: {e}", i + 1))?;
        }
    }
    lines.clone_from_slice(&out);
    Ok(differing)
}

/// The word index to split `text` at so the two halves are as close to
/// equal length (in characters) as possible - the first half gets words
/// `..k`, the second `k..`. `None` for a single word (nothing to split).
pub fn best_split_word(text: &str) -> Option<usize> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 2 {
        return None;
    }
    let len = |ws: &[&str]| {
        ws.iter().map(|w| w.chars().count()).sum::<usize>() + ws.len().saturating_sub(1)
    };
    (1..words.len()).min_by_key(|&k| len(&words[..k]).abs_diff(len(&words[k..])))
}

/// Splits line `idx` into two lines before whole word `at_word` (see
/// [`best_split_word`] - a word, never mid-syllable), keeping its timing:
/// the first half keeps the original start and ends at `split_time`, the
/// second starts at `split_time` and keeps the original end, and every
/// per-unit override goes with its syllable. `split_time` should be when
/// that word's first unit (see [`first_unit_of_word`]) is highlighted
/// (manual or estimated) - pass `None` for an untimed line.
/// Voice, custom name, backing-track membership and block grouping carry
/// over (the second half continues the same verse block).
///
/// `raw` (the pasted lyrics text) gets the same split, so the next re-parse
/// keeps both halves - which needs `raw` to still parse to exactly `lines`'
/// texts; if it doesn't (edited but not re-parsed), nothing changes and
/// an error asks the user to re-parse first.
pub fn split_line(
    lines: &mut Vec<LyricLine>,
    raw: &mut String,
    idx: usize,
    at_word: usize,
    split_time: Option<f64>,
) -> Result<(), String> {
    let Some(orig) = lines.get(idx).cloned() else {
        return Err("No such line.".to_string());
    };
    let words: Vec<&str> = orig.text.split_whitespace().collect();
    if at_word == 0 || at_word >= words.len() {
        return Err("Can't split a line there.".to_string());
    }
    let sources = parse_pasted_lyrics_with_sources(raw);
    let raw_in_sync = sources.len() == lines.len()
        && sources
            .iter()
            .zip(lines.iter())
            .all(|((p, _), l)| p.text == l.text);
    if !raw_in_sync {
        return Err(
            "The lyrics box has edits that haven't been parsed yet - click Parse lyrics \
             first, then split."
                .to_string(),
        );
    }
    let first_text = words[..at_word].join(" ");
    let second_text = words[at_word..].join(" ");
    let split_time = split_time.filter(|_| orig.start.is_some());
    // Overrides are per timing unit (syllable), so split them there.
    let unit_count = timing_units(&orig.text).len();
    let at_unit = first_unit_of_word(&orig.text, at_word);

    let mut first = orig.clone();
    first.text = first_text.clone();
    first.word_overrides = pad_to(&orig.word_overrides, unit_count)[..at_unit].to_vec();
    first.word_end_overrides = pad_to(&orig.word_end_overrides, unit_count)[..at_unit].to_vec();
    first.estimated_sing_duration = None;
    first.backing_vocal = None;
    if split_time.is_some() {
        first.sing_end_override = split_time;
    }

    let mut second = orig.clone();
    second.text = second_text.clone();
    second.word_overrides = pad_to(&orig.word_overrides, unit_count)[at_unit..].to_vec();
    second.word_end_overrides = pad_to(&orig.word_end_overrides, unit_count)[at_unit..].to_vec();
    second.estimated_sing_duration = None;
    second.backing_vocal = None;
    second.starts_new_block = false;
    second.countdown_mode = CountdownMode::default();
    second.start = split_time;
    if split_time.is_none() {
        second.start = None;
        second.sing_end_override = None;
    }

    lines[idx] = first;
    lines.insert(idx + 1, second);

    let raw_idx = sources[idx].1;
    let trailing_newline = raw.ends_with('\n');
    let mut raw_lines: Vec<String> = raw.lines().map(str::to_string).collect();
    raw_lines[raw_idx] = first_text;
    raw_lines.insert(raw_idx + 1, second_text);
    *raw = raw_lines.join("\n");
    if trailing_newline {
        raw.push('\n');
    }
    Ok(())
}

/// `v` padded with `None` (or truncated) to exactly `n` slots - word
/// override vectors should already match the word count, but a split
/// shouldn't panic if one somehow doesn't.
fn pad_to(v: &[Option<f64>], n: usize) -> Vec<Option<f64>> {
    let mut out = v.to_vec();
    out.resize(n, None);
    out
}

/// Index of the nearest main-track line before `idx` - see [`prev_main_line`].
fn prev_main_index(lines: &[LyricLine], idx: usize) -> Option<usize> {
    (0..idx).rev().find(|&i| !lines[i].is_backing)
}

/// Index of the nearest main-track line after `idx` - see [`next_main_line`].
fn next_main_index(lines: &[LyricLine], idx: usize) -> Option<usize> {
    (idx + 1..lines.len()).find(|&i| !lines[i].is_backing)
}

/// Sets `lines[idx]`'s start from a live tap at `pos`. Unlike typing or
/// dragging (which go through [`check_start_change`] and are simply
/// rejected), a live tap can't be retried - the moment has passed - so a
/// tap landing before the previous main line's explicit end (which only
/// happens after seeking back, or when re-tapping between already-timed
/// lines) is kept, and that previous line's end is pulled back to meet it
/// instead. Returns the index of the line whose end was trimmed, if any.
/// Still an error if the tap is before the previous line's own *start*
/// (trimming can't help there) or not before this line's own explicit end.
pub fn apply_tap_start(
    lines: &mut [LyricLine],
    idx: usize,
    pos: f64,
) -> Result<Option<usize>, String> {
    if !pos.is_finite() || pos < 0.0 {
        return Err("Start time can't be negative.".to_string());
    }
    if let Some(end) = lines[idx].sing_end_override {
        if pos >= end {
            return Err(format!(
                "Start ({}) must be before this line's own end ({}).",
                format_timecode(pos),
                format_timecode(end)
            ));
        }
    }
    let mut trimmed = None;
    if !lines[idx].is_backing {
        if let Some(p) = prev_main_index(lines, idx) {
            if lines[p].sing_end_override.is_some_and(|e| pos < e) {
                if let Some(prev_start) = lines[p].start.filter(|&s| pos <= s) {
                    return Err(format!(
                        "Start ({}) can't be before the previous line starts ({}).",
                        format_timecode(pos),
                        format_timecode(prev_start)
                    ));
                }
                lines[p].sing_end_override = Some(pos);
                trimmed = Some(p);
            }
        }
    }
    lines[idx].start = Some(pos);
    Ok(trimmed)
}

/// Sets `lines[idx]`'s end from a live tap at `pos` - the tap-along
/// counterpart of [`check_end_change`]. A tap landing after the next main
/// line's already-set start is clamped to that start instead of being
/// rejected (see [`apply_tap_start`] for why a live tap shouldn't just be
/// dropped). Returns the end actually set, and whether it was clamped.
pub fn apply_tap_end(lines: &mut [LyricLine], idx: usize, pos: f64) -> Result<(f64, bool), String> {
    let Some(start) = lines[idx].start else {
        return Err("Set this line's start before its end.".to_string());
    };
    if !pos.is_finite() || pos <= start {
        return Err(format!(
            "End ({}) must be after this line's start ({}).",
            format_timecode(pos),
            format_timecode(start)
        ));
    }
    let mut end = pos;
    let mut clamped = false;
    if !lines[idx].is_backing {
        if let Some(next_start) = next_main_index(lines, idx).and_then(|n| lines[n].start) {
            if pos > next_start {
                if next_start <= start {
                    return Err(format!(
                        "End ({}) can't be after the next line starts ({}).",
                        format_timecode(pos),
                        format_timecode(next_start)
                    ));
                }
                end = next_start;
                clamped = true;
            }
        }
    }
    lines[idx].sing_end_override = Some(end);
    Ok((end, clamped))
}

/// Converts any legacy embedded "Echo" backing vocals (see [`BackingVocal`])
/// in `lines` into independent backing-track lines ([`LyricLine::is_backing`]),
/// each inserted right after its old host line with its timing, voice and
/// word overrides intact. Also inserts each one's text into `raw` (the
/// pasted-lyrics text) right after its host's own text line, so the next
/// re-parse keeps it rather than dropping it as a line that's no longer in
/// the text - but only when `raw` still parses to exactly `lines`' own
/// texts (it normally does); otherwise `raw` is left alone rather than
/// guessing where to put them. Returns how many were converted.
pub fn migrate_legacy_backing_vocals(lines: &mut Vec<LyricLine>, raw: &mut String) -> usize {
    if lines.iter().all(|l| l.backing_vocal.is_none()) {
        return 0;
    }
    let sources = parse_pasted_lyrics_with_sources(raw);
    let raw_in_sync = sources.len() == lines.len()
        && sources
            .iter()
            .zip(lines.iter())
            .all(|((p, _), l)| p.text == l.text);
    let mut raw_lines: Vec<String> = raw.lines().map(str::to_string).collect();

    let mut converted = 0;
    for host_idx in (0..lines.len()).rev() {
        let Some(bv) = lines[host_idx].backing_vocal.take() else {
            continue;
        };
        let text = bv.text.trim();
        if text.is_empty() {
            continue;
        }
        let mut line = LyricLine::new(text);
        line.is_backing = true;
        line.starts_new_block = false;
        line.start = bv.start;
        line.sing_end_override = bv.end;
        line.singer = bv.singer;
        if bv.word_overrides.len() == line.word_overrides.len() {
            line.word_overrides = bv.word_overrides;
        }
        if bv.word_end_overrides.len() == line.word_end_overrides.len() {
            line.word_end_overrides = bv.word_end_overrides;
        }
        lines.insert(host_idx + 1, line);
        if raw_in_sync {
            raw_lines.insert(sources[host_idx].1 + 1, text.to_string());
        }
        converted += 1;
    }
    if raw_in_sync && converted > 0 {
        let trailing_newline = raw.ends_with('\n');
        *raw = raw_lines.join("\n");
        if trailing_newline {
            raw.push('\n');
        }
    }
    converted
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
    /// See [`LyricLine::countdown_mode`].
    pub countdown_mode: CountdownMode,
    /// See [`LyricLine::custom_singer_name`].
    pub custom_singer_name: Option<String>,
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
                let est = line
                    .estimated_sing_duration
                    .unwrap_or_else(|| estimate_sing_duration(&line.text, window, settings));
                (start + est).clamp(start, end)
            }
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
            countdown_mode: line.countdown_mode,
            custom_singer_name: line.custom_singer_name.clone(),
        }
    }
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

/// Resolve start/end windows for every *main-track* line (backing lines -
/// see [`LyricLine::is_backing`] - are left out; see
/// [`resolve_backing_track`] for those). Lines without a `start` are
/// skipped (caller should validate this first). Lines are sorted by start
/// time. The final line's end is `total_duration` (or `start + 4.0` if
/// `total_duration` is unknown / shorter than that).
pub fn resolve_timing_with_settings(
    lines: &[LyricLine],
    total_duration: Option<f64>,
    settings: &TimingSettings,
) -> Vec<TimedLine> {
    resolve_timing_indexed(lines, total_duration, settings).0
}

/// Same as [`resolve_timing_with_settings`], but also returns each
/// `TimedLine`'s index into `lines` (needed by anything that edits the
/// original line a resolved one came from, since the result is sorted by
/// start time and skips backing/untimed lines).
pub fn resolve_timing_indexed(
    lines: &[LyricLine],
    total_duration: Option<f64>,
    settings: &TimingSettings,
) -> (Vec<TimedLine>, Vec<usize>) {
    // A backing line sitting between two verses can carry the blank-line
    // verse break that really belongs to the next main line (e.g. a blank
    // line, then a backing line, then the next verse's first main line) -
    // hand it on to that main line, so leaving backing lines out of the
    // main track never merges two on-screen blocks into one.
    let mut new_block = vec![false; lines.len()];
    let mut pending_break = false;
    for (i, l) in lines.iter().enumerate() {
        if l.is_backing {
            pending_break |= l.starts_new_block;
        } else {
            new_block[i] = l.starts_new_block || pending_break;
            pending_break = false;
        }
    }

    let mut sorted: Vec<(usize, f64)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.is_backing)
        .filter_map(|(i, l)| l.start.map(|s| (i, s)))
        .collect();
    sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut out = Vec::with_capacity(sorted.len());
    let mut indices = Vec::with_capacity(sorted.len());
    for pos in 0..sorted.len() {
        let (orig_idx, start) = sorted[pos];
        let end = if pos + 1 < sorted.len() {
            sorted[pos + 1].1
        } else {
            match total_duration {
                Some(d) if d > start => d,
                _ => start + 4.0,
            }
        };
        let mut timed = TimedLine::from_line_with_settings(&lines[orig_idx], start, end, settings);
        timed.starts_new_block = new_block[orig_idx];
        out.push(timed);
        indices.push(orig_idx);
    }
    (out, indices)
}

/// Resolves the *backing track* - every timed [`LyricLine::is_backing`]
/// line, sorted by start. Unlike the main track, a backing line's window
/// doesn't depend on its neighbors at all: it runs from its own `start` to
/// its own [`LyricLine::sing_end_override`] (or, until that's tapped, the
/// usual word-count estimate), overlapping whatever main line(s) happen to
/// be playing. `end` and `sing_end` are always equal here - a backing line
/// has no "musical break" tail of its own; see [`backing_display_windows`]
/// for how long it actually stays on screen.
pub fn resolve_backing_track(lines: &[LyricLine], settings: &TimingSettings) -> Vec<TimedLine> {
    resolve_backing_track_indexed(lines, settings).0
}

/// Same as [`resolve_backing_track`], plus each result's index into `lines`.
pub fn resolve_backing_track_indexed(
    lines: &[LyricLine],
    settings: &TimingSettings,
) -> (Vec<TimedLine>, Vec<usize>) {
    let mut sorted: Vec<(usize, f64)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_backing)
        .filter_map(|(i, l)| l.start.map(|s| (i, s)))
        .collect();
    sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut out = Vec::with_capacity(sorted.len());
    let mut indices = Vec::with_capacity(sorted.len());
    for (orig_idx, start) in sorted {
        let line = &lines[orig_idx];
        let end = match line.sing_end_override {
            Some(e) if e > start => e,
            _ => {
                let est = line
                    .estimated_sing_duration
                    .unwrap_or_else(|| estimate_sing_duration(&line.text, f64::INFINITY, settings));
                start + est.max(0.05)
            }
        };
        let mut timed = TimedLine::from_line_with_settings(line, start, end, settings);
        timed.starts_new_block = true;
        out.push(timed);
        indices.push(orig_idx);
    }
    (out, indices)
}

/// How long before a backing line starts that it appears on screen (in its
/// unsung color), so the backing singer can read ahead - see
/// [`backing_display_windows`].
pub const BACKING_LEAD_SECS: f64 = 1.5;
/// How long a backing line stays on screen, fully highlighted, after it
/// finishes - see [`backing_display_windows`].
pub const BACKING_LINGER_SECS: f64 = 1.0;

/// The `[show, hide)` on-screen window for each line of a resolved backing
/// track (as returned by [`resolve_backing_track`], i.e. sorted by start).
/// The backing track gets a single on-screen lane in every renderer
/// (`.cdg`, video, live preview), so these never overlap each other: each
/// line normally shows [`BACKING_LEAD_SECS`] early and lingers
/// [`BACKING_LINGER_SECS`] after it ends, but the lead-in waits for the
/// previous backing line to finish, and the linger is cut short the moment
/// the next backing line needs the lane. If two backing lines genuinely
/// overlap each other, the later one takes the lane over at its own start.
/// Shared by all three renderers so they agree exactly.
pub fn backing_display_windows(track: &[TimedLine]) -> Vec<(f64, f64)> {
    let shows: Vec<f64> = track
        .iter()
        .enumerate()
        .map(|(k, line)| {
            let earliest = match k {
                0 => line.start - BACKING_LEAD_SECS,
                _ => (line.start - BACKING_LEAD_SECS).max(track[k - 1].sing_end),
            };
            earliest.min(line.start).max(0.0)
        })
        .collect();
    track
        .iter()
        .enumerate()
        .map(|(k, line)| {
            let mut hide = line.sing_end + BACKING_LINGER_SECS;
            if let Some(&next_show) = shows.get(k + 1) {
                hide = hide.min(next_show);
            }
            (shows[k], hide.max(shows[k]))
        })
        .collect()
}

/// Which backing-track line (index into `track`) occupies the backing lane
/// at time `t`, if any - see [`backing_display_windows`].
pub fn active_backing_line(windows: &[(f64, f64)], t: f64) -> Option<usize> {
    windows
        .iter()
        .position(|&(show, hide)| t >= show && t < hide)
}

/// Opacity for the active backing line at `t`, given its display window:
/// fades in over its lead-in (see [`reveal_fade_alpha`]) and fades back
/// out over its linger once it's done being sung, so the lane never pops.
/// Video export/live preview only, same as the main track's own fades.
pub fn backing_line_alpha(line: &TimedLine, window: (f64, f64), t: f64) -> f32 {
    if t >= line.sing_end {
        let span = (window.1 - line.sing_end).max(0.001);
        (1.0 - (t - line.sing_end) / span).clamp(0.0, 1.0) as f32
    } else if t < line.start {
        reveal_fade_alpha(window.0, t).max(0.15)
    } else {
        1.0
    }
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
    /// Whether this unit is the last syllable of its word (always true for
    /// a word with no [`SYLLABLE_MARK`]s) - exporters that write text per
    /// unit (LRC2, ASS, UltraStar) put a space after it only when true.
    pub ends_word: bool,
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

/// The shared math behind [`word_timings`] -
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
    let units = timing_units(text);
    let words: Vec<&str> = units.iter().map(|(u, _)| *u).collect();
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
            ends_word: units[i].1,
        });
    }
    out
}

/// Marks a syllable break inside a word in the lyrics: `beau|ti|ful` is
/// one word on screen ("beautiful") but three timing units, each with its
/// own highlight time, so a long held word can wipe syllable by syllable.
/// Chosen over `-` (real hyphenated words like "well-known" must keep their
/// hyphens) and `/` ("and/or"); `|` essentially never appears in lyrics.
pub const SYLLABLE_MARK: char = '|';

/// The timing units of a line's text, in order: whitespace-separated
/// words, further split at [`SYLLABLE_MARK`]s. Each unit comes with whether
/// it ends its word (`false` for every syllable but a word's last). This is
/// what per-"word" timing (`word_overrides`, tapping, the wipe) is indexed
/// by - a word with no syllable marks is simply one unit.
pub fn timing_units(text: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    for word in text.split_whitespace() {
        let parts: Vec<&str> = word
            .split(SYLLABLE_MARK)
            .filter(|p| !p.is_empty())
            .collect();
        let n = parts.len();
        for (k, part) in parts.into_iter().enumerate() {
            out.push((part, k + 1 == n));
        }
    }
    out
}

/// The text as displayed: words re-joined with a single space and
/// [`SYLLABLE_MARK`]s removed - this is what's actually measured and drawn,
/// and what [`word_char_spans`] offsets point into, so they always agree
/// regardless of whatever whitespace was in the source text.
pub fn normalize_text(s: &str) -> String {
    s.split_whitespace()
        .map(|w| w.replace(SYLLABLE_MARK, ""))
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// (char offset, char length) of each timing unit (see [`timing_units`])
/// within the *displayed* text ([`normalize_text`] of `raw_text`). Takes
/// the raw line text, since the displayed text no longer has the syllable
/// marks needed to find syllable boundaries.
pub fn word_char_spans(raw_text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut offset = 0usize;
    for (unit, ends_word) in timing_units(raw_text) {
        let len = unit.chars().count();
        spans.push((offset, len));
        offset += len + usize::from(ends_word);
    }
    spans
}

/// Expands per-*word* `(start, end)` times (one per whitespace word of
/// `text` - e.g. from auto-align, which aligns whole words) into per-unit
/// times (see [`timing_units`]): a word split into syllables has its span
/// divided among them in proportion to their letter counts. Returns one
/// entry per unit; words beyond `word_times` get none (the result is
/// shorter), the same as a short alignment result.
pub fn spread_word_timing(text: &str, word_times: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for (word, &(start, end)) in text.split_whitespace().zip(word_times) {
        let parts: Vec<usize> = word
            .split(SYLLABLE_MARK)
            .filter(|p| !p.is_empty())
            .map(|p| p.chars().count().max(1))
            .collect();
        let total = parts.iter().sum::<usize>().max(1) as f64;
        let mut acc = 0usize;
        for len in parts {
            let a = start + (end - start) * acc as f64 / total;
            acc += len;
            let b = start + (end - start) * acc as f64 / total;
            out.push((a, b));
        }
    }
    out
}

/// Index of the first timing unit of word `word_idx` (whitespace words),
/// i.e. how many units the words before it have.
pub fn first_unit_of_word(text: &str, word_idx: usize) -> usize {
    text.split_whitespace()
        .take(word_idx)
        .map(|w| w.split(SYLLABLE_MARK).filter(|p| !p.is_empty()).count())
        .sum()
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
    resolve_wipe_fraction(&line.text, &word_timings(line), t)
}

/// The shared math behind [`current_line_wipe_fraction`]. `raw_text` is
/// the line's text with any syllable marks (see [`word_char_spans`]).
fn resolve_wipe_fraction(raw_text: &str, words: &[TimedWord], t: f64) -> f32 {
    let char_count = normalize_text(raw_text).chars().count().max(1) as f32;
    let spans = word_char_spans(raw_text);
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
        assert!(singer_legend(&lines, &[]).is_empty());
    }

    #[test]
    fn singer_legend_is_empty_for_no_lines() {
        assert!(singer_legend(&[], &[]).is_empty());
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
        assert!(singer_legend(&lines, &[]).is_empty());
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
        assert_eq!(
            singer_legend(&lines, &[]),
            vec![(Singer::Default, None), (Singer::Female, None)]
        );
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
            singer_legend(&lines, &[]),
            vec![
                (Singer::Male, None),
                (Singer::Female, None),
                (Singer::Screaming, None)
            ]
        );
    }

    #[test]
    fn singer_legend_treats_distinct_custom_names_sharing_one_base_as_separate_entries() {
        // "Male 1" and "Male 2" both use Singer::Male (see this feature's
        // own design: custom names share their base's color, no separate
        // color budget) - the legend should still list them as two
        // distinct entries, not collapse them into one "Male".
        let mut a = TimedLine::new("a".into(), 0.0, 1.0, Singer::Male);
        a.custom_singer_name = Some("Male 1".to_string());
        let mut b = TimedLine::new("b".into(), 1.0, 2.0, Singer::Male);
        b.custom_singer_name = Some("Male 2".to_string());
        let lines = vec![a, b];
        assert_eq!(
            singer_legend(&lines, &[]),
            vec![
                (Singer::Male, Some("Male 1".to_string())),
                (Singer::Male, Some("Male 2".to_string())),
            ]
        );
    }

    #[test]
    fn effective_singer_label_prefers_the_custom_name_when_set() {
        assert_eq!(
            effective_singer_label(Singer::Male, &Some("Male 1".to_string())),
            "Male 1"
        );
        assert_eq!(effective_singer_label(Singer::Male, &None), "Male");
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
    fn sung_line_fade_alpha_stays_full_during_the_linger_then_fades_to_zero() {
        let sing_end = 10.0;
        assert_eq!(sung_line_fade_alpha(sing_end, sing_end), 1.0);
        assert_eq!(
            sung_line_fade_alpha(sing_end, sing_end + SUNG_FADE_LINGER_SECS),
            1.0
        );
        let midway = sing_end + SUNG_FADE_LINGER_SECS + SUNG_FADE_DURATION_SECS / 2.0;
        let alpha = sung_line_fade_alpha(sing_end, midway);
        assert!(
            alpha > 0.0 && alpha < 1.0,
            "expected a partial fade midway through, got {alpha}"
        );
        let after = sing_end + SUNG_FADE_LINGER_SECS + SUNG_FADE_DURATION_SECS + 1.0;
        assert_eq!(sung_line_fade_alpha(sing_end, after), 0.0);
    }

    #[test]
    fn reveal_fade_alpha_ramps_from_zero_to_one() {
        let reveal_at = 5.0;
        assert_eq!(reveal_fade_alpha(reveal_at, reveal_at), 0.0);
        assert_eq!(reveal_fade_alpha(reveal_at, reveal_at - 1.0), 0.0);
        let midway = reveal_at + REVEAL_FADE_DURATION_SECS / 2.0;
        let alpha = reveal_fade_alpha(reveal_at, midway);
        assert!(
            alpha > 0.0 && alpha < 1.0,
            "expected a partial reveal midway through, got {alpha}"
        );
        assert_eq!(
            reveal_fade_alpha(reveal_at, reveal_at + REVEAL_FADE_DURATION_SECS),
            1.0
        );
        assert_eq!(
            reveal_fade_alpha(reveal_at, reveal_at + REVEAL_FADE_DURATION_SECS + 5.0),
            1.0
        );
    }

    #[test]
    fn slot_fade_alpha_matches_sung_line_fade_alpha_once_past_sing_end() {
        let line = TimedLine::new("hi".into(), 0.0, 10.0, Singer::Male);
        // Still actively singing (or hasn't started) - full opacity
        // regardless of the fade curve.
        assert_eq!(slot_fade_alpha(&line, line.sing_end - 0.01), 1.0);
        // Once past sing_end, matches the fade curve exactly.
        let t = line.sing_end + SUNG_FADE_LINGER_SECS + 0.5;
        assert_eq!(
            slot_fade_alpha(&line, t),
            sung_line_fade_alpha(line.sing_end, t)
        );
    }

    #[test]
    fn rolling_preview_lines_borrows_across_a_short_gap_and_stops_at_a_long_one() {
        // Lines 0-3 form one block (a real gap between blocks is the whole
        // point of this test, so "d" -> "e" deliberately crosses a block
        // boundary with only a 0.5s gap - short enough that no countdown
        // would trigger for it). "e" -> "f" is the same. "f" -> "g" is an
        // 11.5s gap - long enough to trigger a real countdown, where the
        // rolling preview must stop rather than reaching past it.
        let mut lines = vec![
            LyricLine::new("a"),
            LyricLine::new("b"),
            LyricLine::new("c"),
            LyricLine::new("d"),
            LyricLine::new("e"),
            LyricLine::new("f"),
            LyricLine::new("g"),
        ];
        let starts = [0.0, 2.0, 4.0, 6.0, 7.0, 8.0, 20.0];
        let sing_ends = [1.0, 3.0, 5.0, 6.5, 7.5, 8.5, 20.5];
        for (i, l) in lines.iter_mut().enumerate() {
            l.start = Some(starts[i]);
            l.sing_end_override = Some(sing_ends[i]);
        }
        for l in &mut lines[1..4] {
            l.starts_new_block = false; // keep 0..=3 in one block
        }
        let timed = resolve_timing(&lines, Some(25.0));
        let settings = TimingSettings::default();
        let block = vec![0, 1, 2, 3];

        assert_eq!(
            rolling_preview_lines(&timed, &block, 0, &settings),
            Vec::<usize>::new()
        );
        assert_eq!(rolling_preview_lines(&timed, &block, 1, &settings), vec![4]);
        assert_eq!(
            rolling_preview_lines(&timed, &block, 2, &settings),
            vec![4, 5]
        );
        // Asking for 3 still only returns 2 - "f" -> "g" is a real
        // countdown-worthy gap, so borrowing stops there instead of
        // reaching past it into "g".
        assert_eq!(
            rolling_preview_lines(&timed, &block, 3, &settings),
            vec![4, 5]
        );
    }

    #[test]
    fn rolling_preview_lines_stops_at_the_end_of_the_song() {
        let mut lines = vec![LyricLine::new("a"), LyricLine::new("b")];
        lines[0].start = Some(0.0);
        lines[0].sing_end_override = Some(1.0);
        lines[1].start = Some(2.0);
        lines[1].sing_end_override = Some(2.5);
        let timed = resolve_timing(&lines, Some(5.0));
        let settings = TimingSettings::default();
        // Block is just line 0 - "b" (idx 1) is the only thing beyond it,
        // and there's nothing after that even though 5 slots were asked for.
        assert_eq!(rolling_preview_lines(&timed, &[0], 5, &settings), vec![1]);
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
    fn estimated_sing_duration_replaces_word_count_estimate_but_not_override() {
        let mut lines = vec![LyricLine::new("a b"), LyricLine::new("next")];
        lines[0].start = Some(10.0);
        lines[1].start = Some(20.0);
        lines[0].estimated_sing_duration = Some(6.0);
        let timed = resolve_timing(&lines, Some(30.0));
        assert_eq!(timed[0].sing_end, 16.0);

        // A manual end still wins over an imported estimate.
        lines[0].sing_end_override = Some(12.0);
        let timed = resolve_timing(&lines, Some(30.0));
        assert_eq!(timed[0].sing_end, 12.0);
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

    fn timed(text: &str, start: f64, end: f64) -> LyricLine {
        let mut l = LyricLine::new(text);
        l.start = Some(start);
        l.sing_end_override = Some(end);
        l
    }

    #[test]
    fn syllable_marks_split_timing_units_but_not_the_displayed_word() {
        let text = "a  beau|ti|ful  day";
        assert_eq!(
            timing_units(text),
            vec![
                ("a", true),
                ("beau", false),
                ("ti", false),
                ("ful", true),
                ("day", true)
            ]
        );
        assert_eq!(normalize_text(text), "a beautiful day");
        // Offsets point into "a beautiful day".
        assert_eq!(
            word_char_spans(text),
            vec![(0, 1), (2, 4), (6, 2), (8, 3), (12, 3)]
        );
        assert_eq!(LyricLine::new(text).word_overrides.len(), 5);
        assert_eq!(first_unit_of_word(text, 2), 4);
        // Stray marks don't make empty units or double spaces.
        assert_eq!(timing_units("|x|| y|"), vec![("x", true), ("y", true)]);
        assert_eq!(normalize_text("| x |"), "x");
        // A word with no marks is still one unit.
        assert_eq!(timing_units("well-known"), vec![("well-known", true)]);
    }

    #[test]
    fn wipe_moves_through_syllables_at_their_own_times() {
        let mut line = LyricLine::new("beau|ti|ful");
        line.word_overrides = vec![Some(0.0), Some(1.0), Some(2.0)];
        let tl = TimedLine::from_line(&line, 0.0, 3.0);
        let mut tl = tl;
        tl.sing_end = 3.0;
        // "beautiful" = 9 chars: "beau" is 4/9, "ti" ends at 6/9.
        let at = |t: f64| current_line_wipe_fraction(&tl, t);
        assert!((at(1.0) - 4.0 / 9.0).abs() < 1e-4, "{}", at(1.0));
        assert!((at(1.5) - 5.0 / 9.0).abs() < 1e-4, "{}", at(1.5));
        assert!((at(2.0) - 6.0 / 9.0).abs() < 1e-4, "{}", at(2.0));
        assert!(word_timings(&tl)
            .iter()
            .map(|w| w.ends_word)
            .eq([false, false, true]));
    }

    #[test]
    fn spread_word_timing_divides_a_word_among_its_syllables() {
        let spread = spread_word_timing("go beau|ti|ful", &[(0.0, 1.0), (1.0, 2.0)]);
        // "beau"/"ti"/"ful" = 4/2/3 of 9 letters.
        let expect = [
            (0.0, 1.0),
            (1.0, 1.0 + 4.0 / 9.0),
            (1.0 + 4.0 / 9.0, 1.0 + 6.0 / 9.0),
            (1.0 + 6.0 / 9.0, 2.0),
        ];
        assert_eq!(spread.len(), 4);
        for (a, b) in spread.iter().zip(expect) {
            assert!(
                (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9,
                "{a:?} vs {b:?}"
            );
        }
    }

    #[test]
    fn adding_syllable_marks_keeps_the_word_start_and_end() {
        let mut old = LyricLine::new("so beautiful now");
        old.word_overrides = vec![Some(1.0), Some(2.0), Some(4.0)];
        old.word_end_overrides = vec![None, Some(3.5), None];
        let new = LyricLine::new("so beau|ti|ful now");
        let (merged, _) = merge_edited_line(&old, &new);
        assert_eq!(
            merged.word_overrides,
            vec![Some(1.0), Some(2.0), None, None, Some(4.0)]
        );
        assert_eq!(
            merged.word_end_overrides,
            vec![None, None, None, Some(3.5), None]
        );
        // And an unchanged syllabified word keeps every syllable's time.
        let mut old = LyricLine::new("beau|ti|ful day");
        old.word_overrides = vec![Some(1.0), Some(1.5), Some(2.0), Some(3.0)];
        let new = LyricLine::new("beau|ti|ful days");
        let (merged, _) = merge_edited_line(&old, &new);
        assert_eq!(
            merged.word_overrides,
            vec![Some(1.0), Some(1.5), Some(2.0), None]
        );
    }

    #[test]
    fn split_line_splits_syllable_overrides_at_the_word_boundary() {
        let mut raw = "beau|ti|ful sunny day".to_string();
        let mut lines = parse_pasted_lyrics(&raw);
        lines[0].start = Some(1.0);
        lines[0].word_overrides = vec![Some(1.0), Some(1.5), Some(2.0), Some(3.0), Some(4.0)];
        split_line(&mut lines, &mut raw, 0, 1, Some(3.0)).unwrap();
        assert_eq!(lines[0].text, "beau|ti|ful");
        assert_eq!(
            lines[0].word_overrides,
            vec![Some(1.0), Some(1.5), Some(2.0)]
        );
        assert_eq!(lines[1].text, "sunny day");
        assert_eq!(lines[1].word_overrides, vec![Some(3.0), Some(4.0)]);
        assert_eq!(raw, "beau|ti|ful\nsunny day");
        // Balancing ignores the marks: "beautiful" (9) | "sunny day" (9).
        assert_eq!(best_split_word("beau|ti|ful sunny day"), Some(1));
    }

    fn chorus_song() -> Vec<LyricLine> {
        let mut lines: Vec<LyricLine> = [
            "Verse one here",
            "Hold me now",
            "Never let go",
            "Verse two here",
            "hold me NOW!",
            "never let go",
            "outro",
        ]
        .iter()
        .map(|t| LyricLine::new(*t))
        .collect();
        // Verse 1 and the first chorus timed.
        lines[0].start = Some(1.0);
        lines[0].sing_end_override = Some(4.0);
        lines[1].start = Some(10.0);
        lines[1].sing_end_override = Some(12.0);
        lines[1].word_overrides = vec![Some(10.0), Some(10.8), Some(11.2)];
        lines[1].word_end_overrides = vec![None, None, Some(11.9)];
        lines[2].start = Some(12.5);
        lines[2].sing_end_override = Some(15.0);
        lines[3].start = Some(20.0);
        lines[3].sing_end_override = Some(23.0);
        lines
    }

    #[test]
    fn find_repeat_matches_text_ignoring_case_and_punctuation() {
        let lines = chorus_song();
        assert_eq!(find_repeat(&lines, &[1, 2]), Some(4));
        // Searching from a later copy finds the earlier one too.
        assert_eq!(find_repeat(&lines, &[4, 5]), Some(1));
        assert_eq!(find_repeat(&lines, &[0]), None);
        assert_eq!(comparable_text("Hold  me NOW!"), "hold me now");
    }

    #[test]
    fn copy_timing_moves_lines_and_words_by_one_offset() {
        let mut lines = chorus_song();
        let differing = copy_timing(&mut lines, &[1, 2], 4, 30.0).unwrap();
        assert_eq!(differing, 0);
        assert_eq!(lines[4].start, Some(30.0));
        assert_eq!(lines[4].sing_end_override, Some(32.0));
        assert_eq!(
            lines[4].word_overrides,
            vec![Some(30.0), Some(30.8), Some(31.2)]
        );
        assert_eq!(lines[4].word_end_overrides, vec![None, None, Some(31.9)]);
        assert_eq!(lines[5].start, Some(32.5));
        assert_eq!(lines[5].sing_end_override, Some(35.0));
        // Text stays the target's own.
        assert_eq!(lines[4].text, "hold me NOW!");
    }

    #[test]
    fn copy_timing_with_different_words_copies_line_timing_only() {
        let mut lines = chorus_song();
        lines[5] = LyricLine::new("never ever let me go");
        let differing = copy_timing(&mut lines, &[1, 2], 4, 30.0).unwrap();
        assert_eq!(differing, 1);
        assert_eq!(lines[5].start, Some(32.5));
        assert_eq!(lines[5].word_overrides, vec![None; 5]);
    }

    #[test]
    fn copy_timing_is_all_or_nothing() {
        let mut lines = chorus_song();
        let before = lines.clone();
        // Would start before verse two (line 3) ends at 23.0.
        assert!(copy_timing(&mut lines, &[1, 2], 4, 22.0).is_err());
        // Overlapping source/target, untimed source, out of range.
        assert!(copy_timing(&mut lines, &[1, 2], 2, 30.0).is_err());
        assert!(copy_timing(&mut lines, &[5, 6], 1, 30.0).is_err());
        assert!(copy_timing(&mut lines, &[1, 2], 6, 30.0).is_err());
        assert_eq!(lines, before);
    }

    #[test]
    fn best_split_word_balances_character_counts() {
        assert_eq!(best_split_word("one"), None);
        assert_eq!(best_split_word("aa bb"), Some(1));
        // "I want to hold" (14) | "your hand tonight" (17)
        assert_eq!(best_split_word("I want to hold your hand tonight"), Some(4));
    }

    #[test]
    fn split_line_keeps_timing_words_and_raw_text_in_sync() {
        let mut raw = "first line\n\nalpha beta gamma delta\nlast\n".to_string();
        let mut lines = parse_pasted_lyrics(&raw);
        lines[1].start = Some(10.0);
        lines[1].sing_end_override = Some(14.0);
        lines[1].word_overrides = vec![Some(10.0), None, Some(12.0), None];
        lines[1].word_end_overrides = vec![None, None, None, Some(13.8)];
        lines[1].singer = Singer::Female;
        assert!(lines[1].starts_new_block);

        split_line(&mut lines, &mut raw, 1, 2, Some(12.0)).unwrap();

        assert_eq!(lines.len(), 4);
        assert_eq!(lines[1].text, "alpha beta");
        assert_eq!(lines[1].start, Some(10.0));
        assert_eq!(lines[1].sing_end_override, Some(12.0));
        assert_eq!(lines[1].word_overrides, vec![Some(10.0), None]);
        assert!(lines[1].starts_new_block);
        assert_eq!(lines[2].text, "gamma delta");
        assert_eq!(lines[2].start, Some(12.0));
        assert_eq!(lines[2].sing_end_override, Some(14.0));
        assert_eq!(lines[2].word_overrides, vec![Some(12.0), None]);
        assert_eq!(lines[2].word_end_overrides, vec![None, Some(13.8)]);
        assert_eq!(lines[2].singer, Singer::Female);
        assert!(!lines[2].starts_new_block);
        // Raw text re-parses to exactly the new lines.
        assert_eq!(raw, "first line\n\nalpha beta\ngamma delta\nlast\n");
        let reparsed: Vec<String> = parse_pasted_lyrics(&raw)
            .into_iter()
            .map(|l| l.text)
            .collect();
        let now: Vec<String> = lines.iter().map(|l| l.text.clone()).collect();
        assert_eq!(reparsed, now);
    }

    #[test]
    fn split_line_untimed_stays_untimed() {
        let mut raw = "a b c d".to_string();
        let mut lines = parse_pasted_lyrics(&raw);
        split_line(&mut lines, &mut raw, 0, 2, Some(5.0)).unwrap();
        assert_eq!(lines[0].start, None);
        assert_eq!(lines[0].sing_end_override, None);
        assert_eq!(lines[1].start, None);
        assert_eq!(raw, "a b\nc d");
    }

    #[test]
    fn split_line_refuses_when_raw_is_out_of_sync() {
        let mut raw = "a b c d".to_string();
        let mut lines = parse_pasted_lyrics(&raw);
        raw.push_str("\nnew unparsed line");
        let before = (lines.clone(), raw.clone());
        assert!(split_line(&mut lines, &mut raw, 0, 2, None).is_err());
        assert_eq!((lines, raw), before);
    }

    #[test]
    fn shift_lines_moves_starts_ends_and_word_overrides_together() {
        let mut lines = vec![timed("a b", 1.0, 3.0), timed("c d", 4.0, 6.0)];
        lines[1].word_overrides = vec![Some(4.0), Some(5.0)];
        lines[1].word_end_overrides = vec![None, Some(5.8)];
        assert_eq!(shift_lines(&mut lines, &[1], 0.5), Ok(1));
        assert_eq!(lines[1].start, Some(4.5));
        assert_eq!(lines[1].sing_end_override, Some(6.5));
        assert_eq!(lines[1].word_overrides, vec![Some(4.5), Some(5.5)]);
        assert_eq!(lines[1].word_end_overrides, vec![None, Some(6.3)]);
        // Unselected line untouched.
        assert_eq!(lines[0].start, Some(1.0));
    }

    #[test]
    fn shift_lines_rejects_overlap_and_changes_nothing() {
        let mut lines = vec![timed("a", 1.0, 3.0), timed("b", 4.0, 6.0)];
        let before = lines.clone();
        assert!(shift_lines(&mut lines, &[1], -1.5).is_err());
        assert_eq!(lines, before);
        // Shifting both together is fine - they keep their spacing.
        assert_eq!(shift_lines(&mut lines, &[0, 1], -0.5), Ok(2));
        assert_eq!(lines[0].start, Some(0.5));
        assert_eq!(lines[1].start, Some(3.5));
    }

    #[test]
    fn shift_lines_rejects_negative_times() {
        let mut lines = vec![timed("a", 0.2, 3.0)];
        assert!(shift_lines(&mut lines, &[0], -0.3).is_err());
        assert_eq!(lines[0].start, Some(0.2));
    }

    #[test]
    fn shift_lines_skips_untimed_and_errors_if_nothing_is_timed() {
        let mut lines = vec![LyricLine::new("a"), timed("b", 4.0, 6.0)];
        assert!(shift_lines(&mut lines, &[0], 1.0).is_err());
        assert_eq!(shift_lines(&mut lines, &[0, 1], 1.0), Ok(1));
        assert_eq!(lines[0].start, None);
    }

    #[test]
    fn tap_start_before_previous_end_trims_it_instead_of_failing() {
        let mut lines = vec![timed("a", 1.0, 5.0), LyricLine::new("b")];
        assert_eq!(apply_tap_start(&mut lines, 1, 4.2), Ok(Some(0)));
        assert_eq!(lines[0].sing_end_override, Some(4.2));
        assert_eq!(lines[1].start, Some(4.2));
    }

    #[test]
    fn tap_start_after_previous_end_leaves_it_alone() {
        let mut lines = vec![timed("a", 1.0, 5.0), LyricLine::new("b")];
        assert_eq!(apply_tap_start(&mut lines, 1, 5.0), Ok(None));
        assert_eq!(lines[0].sing_end_override, Some(5.0));
    }

    #[test]
    fn tap_start_before_previous_start_is_still_an_error() {
        let mut lines = vec![timed("a", 3.0, 5.0), LyricLine::new("b")];
        assert!(apply_tap_start(&mut lines, 1, 2.0).is_err());
        assert!(apply_tap_start(&mut lines, 1, 3.0).is_err());
        assert_eq!(lines[0].sing_end_override, Some(5.0));
        assert_eq!(lines[1].start, None);
    }

    #[test]
    fn tap_start_skips_backing_lines_for_the_previous_line() {
        let mut backing = timed("bg", 2.0, 9.0);
        backing.is_backing = true;
        let mut lines = vec![timed("a", 1.0, 5.0), backing, LyricLine::new("b")];
        assert_eq!(apply_tap_start(&mut lines, 2, 4.0), Ok(Some(0)));
        // The backing line is free to overlap, so it's untouched.
        assert_eq!(lines[1].sing_end_override, Some(9.0));
    }

    #[test]
    fn tap_end_after_next_start_clamps_to_it() {
        let mut lines = vec![LyricLine::new("a"), timed("b", 5.0, 8.0)];
        lines[0].start = Some(1.0);
        assert_eq!(apply_tap_end(&mut lines, 0, 5.4), Ok((5.0, true)));
        assert_eq!(lines[0].sing_end_override, Some(5.0));
        assert_eq!(apply_tap_end(&mut lines, 0, 4.0), Ok((4.0, false)));
    }

    #[test]
    fn tap_end_errors_without_a_start_or_before_it() {
        let mut lines = vec![LyricLine::new("a")];
        assert!(apply_tap_end(&mut lines, 0, 2.0).is_err());
        lines[0].start = Some(3.0);
        assert!(apply_tap_end(&mut lines, 0, 3.0).is_err());
        assert_eq!(lines[0].sing_end_override, None);
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

    // --- backing track -----------------------------------------------------

    fn backing(text: &str, start: Option<f64>, end: Option<f64>) -> LyricLine {
        let mut l = LyricLine::new(text);
        l.is_backing = true;
        l.start = start;
        l.sing_end_override = end;
        l
    }

    #[test]
    fn backing_lines_are_left_out_of_the_main_track() {
        let mut lines = vec![
            LyricLine::new("main one"),
            backing("scream over it", Some(1.0), Some(9.0)),
            LyricLine::new("main two"),
        ];
        lines[0].start = Some(0.0);
        lines[2].start = Some(5.0);
        let (timed, indices) =
            resolve_timing_indexed(&lines, Some(20.0), &TimingSettings::default());
        assert_eq!(indices, vec![0, 2]);
        // The main line's end is the next *main* line's start, not the
        // backing line's start in between.
        assert_eq!(timed[0].end, 5.0);
    }

    #[test]
    fn the_backing_track_overlaps_main_lines_freely() {
        let mut lines = vec![
            LyricLine::new("main one"),
            backing("scream over it", Some(1.0), Some(9.0)),
            LyricLine::new("main two"),
        ];
        lines[0].start = Some(0.0);
        lines[2].start = Some(5.0);
        let (track, indices) = resolve_backing_track_indexed(&lines, &TimingSettings::default());
        assert_eq!(indices, vec![1]);
        assert_eq!(track[0].start, 1.0);
        // Spans past main line 1's own end, into main line 2 - the whole point.
        assert_eq!(track[0].sing_end, 9.0);
        assert_eq!(track[0].end, 9.0);
    }

    #[test]
    fn an_untapped_backing_end_falls_back_to_the_word_estimate() {
        let lines = vec![backing("one two three four", Some(10.0), None)];
        let track = resolve_backing_track(&lines, &TimingSettings::default());
        let expected = 4.0 * SECONDS_PER_WORD;
        assert!((track[0].sing_end - (10.0 + expected)).abs() < 1e-9);
    }

    #[test]
    fn an_untimed_backing_line_is_not_resolved_at_all() {
        let lines = vec![backing("not yet", None, None)];
        assert!(resolve_backing_track(&lines, &TimingSettings::default()).is_empty());
    }

    #[test]
    fn a_backing_lines_verse_break_is_handed_on_to_the_next_main_line() {
        // "a", blank line, backing "x", then main "b" - "b" itself parsed
        // with starts_new_block = false (it directly follows "x"), but it
        // must still start a new block once "x" is out of the main track.
        let mut lines = parse_pasted_lyrics("a\n\nx\nb");
        lines[1].is_backing = true;
        lines[0].start = Some(0.0);
        lines[1].start = Some(1.0);
        lines[2].start = Some(2.0);
        assert!(!lines[2].starts_new_block);
        let timed = resolve_timing(&lines, Some(10.0));
        assert_eq!(group_into_blocks(&timed), vec![vec![0], vec![1]]);
    }

    #[test]
    fn main_line_neighbor_checks_skip_over_backing_lines() {
        let mut lines = vec![
            LyricLine::new("a"),
            backing("x", Some(2.0), Some(8.0)),
            LyricLine::new("b"),
        ];
        lines[0].start = Some(0.0);
        lines[0].sing_end_override = Some(4.0);
        lines[2].start = Some(5.0);
        // "b"'s previous *main* line is "a" (ends at 4.0), not "x" (ends 8.0).
        assert!(check_start_change(&lines, 2, 4.5).is_ok());
        assert!(check_start_change(&lines, 2, 3.0).is_err());
        // "a"'s next main line is "b" (starts 5.0), not "x" (starts 2.0).
        assert!(check_end_change(&lines, 0, 4.9).is_ok());
        assert!(check_end_change(&lines, 0, 5.1).is_err());
    }

    #[test]
    fn backing_lines_have_no_neighbor_rule_only_their_own_start_before_end() {
        let mut lines = vec![
            LyricLine::new("a"),
            backing("x", Some(2.0), Some(8.0)),
            LyricLine::new("b"),
        ];
        lines[0].start = Some(0.0);
        lines[0].sing_end_override = Some(4.0);
        lines[2].start = Some(5.0);
        assert!(check_start_change(&lines, 1, 0.5).is_ok());
        assert!(check_start_change(&lines, 1, 8.0).is_err()); // not before its own end
        assert!(check_end_change(&lines, 1, 30.0).is_ok());
        assert!(check_end_change(&lines, 1, 2.0).is_err()); // not after its own start
    }

    #[test]
    fn backing_display_windows_lead_in_linger_and_never_overlap() {
        let lines = vec![
            backing("first", Some(10.0), Some(12.0)),
            backing("second", Some(12.5), Some(14.0)),
            backing("third", Some(30.0), Some(31.0)),
        ];
        let track = resolve_backing_track(&lines, &TimingSettings::default());
        let w = backing_display_windows(&track);
        assert_eq!(w[0].0, 10.0 - BACKING_LEAD_SECS);
        // Second can't show before first is done singing (12.0), so first's
        // linger is cut short right there.
        assert_eq!(w[1].0, 12.0);
        assert_eq!(w[0].1, 12.0);
        assert_eq!(w[1].1, 14.0 + BACKING_LINGER_SECS);
        assert_eq!(w[2], (30.0 - BACKING_LEAD_SECS, 31.0 + BACKING_LINGER_SECS));
        for pair in w.windows(2) {
            assert!(pair[0].1 <= pair[1].0);
        }
        assert_eq!(active_backing_line(&w, 11.0), Some(0));
        assert_eq!(active_backing_line(&w, 13.0), Some(1));
        assert_eq!(active_backing_line(&w, 20.0), None);
    }

    #[test]
    fn overlapping_backing_lines_hand_the_lane_over_at_the_later_ones_start() {
        let lines = vec![
            backing("long one", Some(10.0), Some(20.0)),
            backing("cuts in", Some(15.0), Some(18.0)),
        ];
        let track = resolve_backing_track(&lines, &TimingSettings::default());
        let w = backing_display_windows(&track);
        assert_eq!(w[1].0, 15.0);
        assert_eq!(w[0].1, 15.0);
    }

    #[test]
    fn a_reparse_keeps_custom_singer_names_and_countdown_modes() {
        let raw = "first line\nsecond line";
        let mut lines = parse_pasted_lyrics(raw);
        lines[0].custom_singer_name = Some("Lead".to_string());
        lines[0].countdown_mode = CountdownMode::Force;
        lines[1].custom_singer_name = Some("Backup".to_string());
        lines[1].countdown_mode = CountdownMode::Suppress;

        // Unchanged text (exact-match anchors).
        let (merged, _) = merge_reparsed_lyrics(&lines, raw, None);
        assert_eq!(merged[0].custom_singer_name.as_deref(), Some("Lead"));
        assert_eq!(merged[0].countdown_mode, CountdownMode::Force);
        assert_eq!(merged[1].custom_singer_name.as_deref(), Some("Backup"));
        assert_eq!(merged[1].countdown_mode, CountdownMode::Suppress);

        // Reworded line (gap pairing), with a different word count too.
        let (merged, report) =
            merge_reparsed_lyrics(&lines, "first line\nsecond line reworded", None);
        assert_eq!(report.reworded_word_count_changed, 1);
        assert_eq!(merged[1].custom_singer_name.as_deref(), Some("Backup"));
        assert_eq!(merged[1].countdown_mode, CountdownMode::Suppress);
    }

    #[test]
    fn a_reparse_keeps_a_line_on_the_backing_track() {
        let raw = "main line\nbacking line";
        let mut lines = parse_pasted_lyrics(raw);
        lines[1].is_backing = true;
        lines[1].start = Some(1.0);
        let (merged, _) = merge_reparsed_lyrics(&lines, raw, None);
        assert!(merged[1].is_backing);
        // Reworded (same word count) - still backing.
        let (merged, report) = merge_reparsed_lyrics(&lines, "main line\nbacking words", None);
        assert_eq!(report.reworded_same_word_count, 1);
        assert!(merged[1].is_backing);
        assert_eq!(merged[1].start, Some(1.0));
        // A newly typed line in between is a plain main line.
        let (merged, _) = merge_reparsed_lyrics(&lines, "main line\nnew one\nbacking line", None);
        assert!(!merged[1].is_backing);
        assert!(merged[2].is_backing);
    }

    #[test]
    fn legacy_echo_vocals_migrate_into_backing_lines_and_the_raw_text() {
        let mut raw = "first line\n\nsecond line\n".to_string();
        let mut lines = parse_pasted_lyrics(&raw);
        let mut bv = BackingVocal::new("echo echo");
        bv.start = Some(2.0);
        bv.end = Some(3.0);
        bv.singer = Singer::Female;
        bv.word_overrides = vec![Some(2.0), Some(2.5)];
        bv.word_end_overrides = vec![None, None];
        lines[0].backing_vocal = Some(bv);

        assert_eq!(migrate_legacy_backing_vocals(&mut lines, &mut raw), 1);
        assert_eq!(raw, "first line\necho echo\n\nsecond line\n");
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| l.backing_vocal.is_none()));
        let m = &lines[1];
        assert!(m.is_backing);
        assert_eq!(m.text, "echo echo");
        assert_eq!((m.start, m.sing_end_override), (Some(2.0), Some(3.0)));
        assert_eq!(m.singer, Singer::Female);
        assert_eq!(m.word_overrides, vec![Some(2.0), Some(2.5)]);
        // And the migrated text re-parses back onto the same lines.
        let (merged, report) = merge_reparsed_lyrics(&lines, &raw, None);
        assert!(report.is_fully_unchanged(3));
        assert!(merged[1].is_backing);
    }

    #[test]
    fn legacy_backing_vocal_json_still_deserializes() {
        let json = r#"{"text":"hi","start":1.0,"singer":"Male","word_overrides":[null],
            "word_end_overrides":[null],"sing_end_override":null,"starts_new_block":true,
            "backing_vocal":{"text":"echo","start":1.5,"end":null,"singer":"Female",
            "word_overrides":[null],"word_end_overrides":[null]}}"#;
        let line: LyricLine = serde_json::from_str(json).unwrap();
        assert!(!line.is_backing);
        assert_eq!(line.backing_vocal.unwrap().text, "echo");
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
        let (merged, report) = merge_reparsed_lyrics(&old, raw, None);

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
        let (merged, report) = merge_reparsed_lyrics(&old, raw, None);

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
        let (merged, report) = merge_reparsed_lyrics(&old, raw, None);

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
        let (merged, report) = merge_reparsed_lyrics(&old, raw, None);

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
        let (merged, report) = merge_reparsed_lyrics(&old, raw, None);

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
        let (merged, report) = merge_reparsed_lyrics(&old, raw, None);

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
        let (merged, _report) = merge_reparsed_lyrics(&old, raw, None);

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
        let (merged, _report) = merge_reparsed_lyrics(&old, raw, None);

        assert!(merged[1].starts_new_block);
        // Timing itself is still preserved even though this flag wasn't.
        assert_eq!(merged[1].start, old[1].start);
    }

    #[test]
    fn empty_old_lines_behaves_like_a_plain_first_parse() {
        let old: Vec<LyricLine> = Vec::new();
        let raw = "a\nb\nc";
        let (merged, report) = merge_reparsed_lyrics(&old, raw, None);
        assert_eq!(merged, parse_pasted_lyrics(raw));
        assert_eq!(report.added, 3);
        assert_eq!(report.unchanged, 0);
    }

    #[test]
    fn clearing_the_text_entirely_drops_every_line_and_its_timing() {
        let old = vec![timed_line("first", 1.0), timed_line("second", 5.0)];
        let (merged, report) = merge_reparsed_lyrics(&old, "", None);
        assert!(merged.is_empty());
        assert_eq!(report.removed, 2);
    }

    #[test]
    fn pasting_a_copy_of_the_following_lines_leaves_the_pasted_copy_untimed() {
        // "y"/"z" copied and pasted in again right after "x" - the text alone
        // reads the same whether the new pair is rows 2-3 or rows 4-5, so the
        // edited region (raw lines 2-3) is what says which copy is new.
        let old = timed_lines_from("w\nx\ny\nz", &[160.0, 175.0, 188.0, 208.0]);
        let raw = "w\nx\ny\nz\ny\nz";
        let (merged, report) = merge_reparsed_lyrics(&old, raw, Some(2..4));

        assert_eq!(report.unchanged, 4);
        assert_eq!(report.added, 2);
        assert_timing_preserved(&merged[0], &old[0]);
        assert_timing_preserved(&merged[1], &old[1]);
        assert_eq!(merged[2].start, None);
        assert_eq!(merged[3].start, None);
        assert_timing_preserved(&merged[4], &old[2]);
        assert_timing_preserved(&merged[5], &old[3]);

        // Pasting the same pair *after* the original instead (raw lines
        // 4-5) still leaves the original pair's timing where it was.
        let (merged, _) = merge_reparsed_lyrics(&old, raw, Some(4..6));
        assert_timing_preserved(&merged[2], &old[2]);
        assert_timing_preserved(&merged[3], &old[3]);
        assert_eq!(merged[4].start, None);
        assert_eq!(merged[5].start, None);
    }

    #[test]
    fn edited_region_never_costs_an_otherwise_matchable_line() {
        // The hint covers every line, but the one genuinely new line is
        // still the only one left untimed - it only breaks ties.
        let old = timed_lines_from("a\nb\nc", &[1.0, 2.0, 3.0]);
        let (merged, report) = merge_reparsed_lyrics(&old, "a\nnew\nb\nc", Some(0..4));
        assert_eq!(report.unchanged, 3);
        assert_eq!(report.added, 1);
        assert_eq!(merged[1].start, None);
        assert_timing_preserved(&merged[2], &old[1]);
    }

    #[test]
    fn lcs_matches_is_a_strictly_increasing_valid_alignment() {
        let old = parse_pasted_lyrics("a\nb\na\nc\nb");
        let new = parse_pasted_lyrics("x\na\nb\na\ny\nb");
        let pairs = lcs_matches(&old, &new, &[]);
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
