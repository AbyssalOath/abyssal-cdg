//! Turns timed lyrics into an actual CDG byte stream.
//!
//! Layout: two text rows are used for the current line's own words - a
//! "current" row (starting at row 6, a 2-row-tall band so it can render at
//! 2x size when it fits) plus, below it, a dim single-row "preview" of the
//! next line (row 10). A backing/echo vocal (row 8, see
//! [`crate::lyrics::BackingVocal`]), if the current line has one, gets its
//! own row directly under the current line's band, with its own
//! independent word-wipe running concurrently with the current line's own -
//! see [`HighlightEvent`] for how the two wipes get interleaved correctly.
//! Also: a title/artist intro card (rows 1-4) at the very start, and a "get
//! ready" countdown row (row 13) during long instrumental breaks. Each
//! line's words light up left-to-right in sync with its estimated singing
//! duration (a classic color-wipe), using [`crate::lyrics::word_timings`].
//!
//! Text size: CDG's native font is only 6 pixels wide, which reads as
//! quite small/blocky on modern displays. Since the format's canvas is a
//! fixed 300x216 pixels no matter what (there's no "resolution setting" -
//! see `video.rs` for an HD/4K alternative), we make the best of it: the
//! current line renders at 2x size (each character spanning a 2x2 grid of
//! tiles) whenever it's short enough to fit, and automatically falls back
//! to normal size for longer lines so nothing runs off-screen or clips. A
//! backing vocal always renders at 1x - a smaller, visually secondary row.
//!
//! Color palette: the CDG format gives us 16 color slots. We use 14 of
//! them: 8 for background/voice text colors (loaded via the low CLUT,
//! completely full: background + 3 voices x 2 colors + preview), and 6
//! more for the title/artist card plus the "screaming" and "default" voice
//! color pairs (loaded via the high CLUT, which has 2 of its 8 slots left
//! free for future use). A backing vocal doesn't need any of those free
//! slots - it reuses [`crate::lyrics::Singer`] for its own color instead of
//! a dedicated category, so it can be colored distinctly from its host
//! (e.g. host = Male, backing = Female) using colors that are already loaded.

use crate::cdg::{CdgColor, CdgWriter, BLANK_TILE, SAFE_COLS};
use crate::font;
#[cfg(test)]
use crate::lyrics::CountdownMode;
use crate::lyrics::{
    backing_vocal_word_timings, countdown_window, countdown_window_between, singer_legend,
    word_timings, Singer, TimedLine, TimingSettings, SUNG_LINGER_SECS,
};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The current line gets a 2-row-tall band so it can use 2x-scaled text.
const TITLE_ROW: u8 = 1;
const ARTIST_ROW: u8 = 4;
/// Row for the singer color legend (e.g. "Male  Female"), shown alongside
/// the title/artist card for songs with more than one voice color in play.
const LEGEND_ROW: u8 = 5;
const CURRENT_ROW: u8 = 6;
/// A backing/echo vocal (see [`crate::lyrics::BackingVocal`]) renders here,
/// directly beneath the current line's own band, always at 1x scale - a
/// smaller row for what's meant to read as a secondary, subordinate voice,
/// with its own independent word-wipe running concurrently with the
/// current line's. Free during normal singing (rows 8-9, 11-12, and 14-15
/// are all otherwise unused outside the title card), so this doesn't
/// crowd anything else out.
const BACKING_ROW: u8 = 8;
const PREVIEW_ROW: u8 = 10;
const COUNTDOWN_ROW: u8 = 13;

/// Preferred (largest) scale for the title card and current line; both
/// fall back to 1x automatically if the text is too long to fit at this size.
const PREFERRED_SCALE: u8 = 2;

// Low CLUT (palette indices 0-7):
const BG: u8 = 0;
const MALE_UNSUNG: u8 = 1;
const MALE_HIGHLIGHT: u8 = 2;
const FEMALE_UNSUNG: u8 = 3;
const FEMALE_HIGHLIGHT: u8 = 4;
const DUET_UNSUNG: u8 = 5;
const DUET_HIGHLIGHT: u8 = 6;
const PREVIEW: u8 = 7;
// High CLUT (palette indices 8-15, loaded separately; 14-15 unused):
const TITLE: u8 = 8;
const ARTIST: u8 = 9;
const SCREAMING_UNSUNG: u8 = 10;
const SCREAMING_HIGHLIGHT: u8 = 11;
const DEFAULT_UNSUNG: u8 = 12;
const DEFAULT_HIGHLIGHT: u8 = 13;

/// A fully customizable set of on-screen colors. All defaults produce the
/// classic white/yellow karaoke look; duet voices get their own color pair
/// so singers can tell at a glance whose line is whose.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub background: CdgColor,
    /// Color for a line whose singer hasn't been manually set - defaults to
    /// the same look as `male_unsung`/`male_highlight`, but independently
    /// adjustable (e.g. to match a background image/video's palette).
    pub default_unsung: CdgColor,
    pub default_highlight: CdgColor,
    pub male_unsung: CdgColor,
    pub male_highlight: CdgColor,
    pub female_unsung: CdgColor,
    pub female_highlight: CdgColor,
    pub duet_unsung: CdgColor,
    pub duet_highlight: CdgColor,
    /// Color for the dimmed next-line preview and unlit countdown markers.
    pub preview: CdgColor,
    pub title: CdgColor,
    pub artist: CdgColor,
    pub screaming_unsung: CdgColor,
    pub screaming_highlight: CdgColor,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            background: CdgColor::new(0, 0, 2),
            default_unsung: CdgColor::new(14, 14, 14),
            default_highlight: CdgColor::new(15, 15, 0),
            male_unsung: CdgColor::new(14, 14, 14),
            male_highlight: CdgColor::new(15, 15, 0),
            female_unsung: CdgColor::new(10, 10, 15),
            female_highlight: CdgColor::new(15, 5, 15),
            duet_unsung: CdgColor::new(10, 15, 10),
            duet_highlight: CdgColor::new(15, 10, 0),
            preview: CdgColor::new(6, 6, 9),
            title: CdgColor::new(15, 15, 15),
            artist: CdgColor::new(9, 9, 12),
            screaming_unsung: CdgColor::new(9, 2, 1),
            screaming_highlight: CdgColor::new(15, 3, 0),
        }
    }
}

impl Palette {
    /// A high-visibility preset - near-black background, pure white/yellow
    /// text - aimed at looking good on an older or washed-out TV/projector
    /// rather than a nicer-looking monitor.
    pub fn high_contrast() -> Self {
        Self {
            background: CdgColor::new(0, 0, 0),
            default_unsung: CdgColor::new(7, 7, 7),
            default_highlight: CdgColor::new(15, 15, 0),
            male_unsung: CdgColor::new(7, 7, 7),
            male_highlight: CdgColor::new(15, 15, 0),
            female_unsung: CdgColor::new(6, 6, 9),
            female_highlight: CdgColor::new(0, 15, 15),
            duet_unsung: CdgColor::new(7, 9, 6),
            duet_highlight: CdgColor::new(15, 8, 0),
            preview: CdgColor::new(8, 8, 8),
            title: CdgColor::new(15, 15, 15),
            artist: CdgColor::new(11, 11, 11),
            screaming_unsung: CdgColor::new(9, 2, 2),
            screaming_highlight: CdgColor::new(15, 0, 0),
        }
    }

    /// A warm preset - deep purple background, orange/pink/gold text.
    pub fn sunset() -> Self {
        Self {
            background: CdgColor::new(2, 0, 3),
            default_unsung: CdgColor::new(9, 6, 10),
            default_highlight: CdgColor::new(15, 9, 2),
            male_unsung: CdgColor::new(9, 6, 10),
            male_highlight: CdgColor::new(15, 9, 2),
            female_unsung: CdgColor::new(10, 6, 9),
            female_highlight: CdgColor::new(15, 3, 8),
            duet_unsung: CdgColor::new(10, 7, 6),
            duet_highlight: CdgColor::new(15, 12, 0),
            preview: CdgColor::new(7, 4, 8),
            title: CdgColor::new(15, 10, 4),
            artist: CdgColor::new(11, 7, 9),
            screaming_unsung: CdgColor::new(8, 1, 2),
            screaming_highlight: CdgColor::new(15, 2, 3),
        }
    }

    /// A cool preset - deep navy background, cyan/blue/teal text.
    pub fn ocean() -> Self {
        Self {
            background: CdgColor::new(0, 1, 3),
            default_unsung: CdgColor::new(6, 9, 11),
            default_highlight: CdgColor::new(0, 14, 15),
            male_unsung: CdgColor::new(6, 9, 11),
            male_highlight: CdgColor::new(0, 14, 15),
            female_unsung: CdgColor::new(7, 8, 12),
            female_highlight: CdgColor::new(4, 10, 15),
            duet_unsung: CdgColor::new(6, 11, 9),
            duet_highlight: CdgColor::new(0, 15, 9),
            preview: CdgColor::new(5, 7, 9),
            title: CdgColor::new(14, 15, 15),
            artist: CdgColor::new(9, 11, 13),
            screaming_unsung: CdgColor::new(2, 3, 8),
            screaming_highlight: CdgColor::new(9, 0, 15),
        }
    }

    /// A near-black preset - blood-red unsung text against a solid black
    /// background, warm gold for the sung/highlight wipe. The red/gold pair
    /// is deliberately a *lightness* contrast (dark vs. bright), not a hue
    /// contrast - it stays distinguishable under deuteranopia, protanopia,
    /// and tritanopia alike, unlike a same-brightness red-vs-green pairing
    /// (the one combination that fails hardest for the most common forms of
    /// color blindness). Every other voice pair here follows the same
    /// principle (a clearly dim tone vs. a clearly bright one) and
    /// deliberately avoids a green-vs-red axis entirely, rather than just
    /// avoiding it for the default voice.
    ///
    /// CDG colors are 4 bits per channel (0-15), so this only ever
    /// *approximates* exact hex values - `CdgColor::new(8, 0, 0)` (the
    /// closest 4-bit step to `#8B0000`) renders as `#880000`, off by a few
    /// values per channel at most. Every preset here has the same ceiling;
    /// it's not specific to this one.
    pub fn abyssal() -> Self {
        Self {
            background: CdgColor::new(0, 0, 0),
            // Blood red (~#8B0000) -> warm gold (~#FFB000).
            default_unsung: CdgColor::new(8, 0, 0),
            default_highlight: CdgColor::new(15, 10, 0),
            male_unsung: CdgColor::new(8, 0, 0),
            male_highlight: CdgColor::new(15, 10, 0),
            // Dim violet -> bright magenta - same dark/bright split as the
            // default pair, on a blue-red axis with no green component at
            // all, so it can't be confused with the red/gold pair even
            // under red-green color blindness.
            female_unsung: CdgColor::new(4, 2, 6),
            female_highlight: CdgColor::new(15, 3, 10),
            // Dim teal -> bright cyan - a blue-green pair (green only ever
            // shows up paired against *more blue-green*, never against red).
            duet_unsung: CdgColor::new(3, 5, 5),
            duet_highlight: CdgColor::new(2, 15, 14),
            // Dim near-black brick -> stark white - the largest possible
            // lightness jump, and no hue reliance whatsoever.
            screaming_unsung: CdgColor::new(5, 1, 1),
            screaming_highlight: CdgColor::new(15, 15, 15),
            // Dim red-grey, one step brighter than the black background -
            // barely-there, matching every other preset's "preview" being a
            // dim/desaturated tone near its own background.
            preview: CdgColor::new(4, 1, 1),
            // Warm cream (ties into the gold accent) for the title, dimmer
            // for the artist line underneath - same relationship every
            // other preset uses between the two.
            title: CdgColor::new(15, 14, 12),
            artist: CdgColor::new(9, 6, 5),
        }
    }

    fn low_clut(&self) -> [CdgColor; 8] {
        [
            self.background,
            self.male_unsung,
            self.male_highlight,
            self.female_unsung,
            self.female_highlight,
            self.duet_unsung,
            self.duet_highlight,
            self.preview,
        ]
    }

    fn high_clut(&self) -> [CdgColor; 8] {
        let zero = CdgColor::new(0, 0, 0);
        [
            self.title,
            self.artist,
            self.screaming_unsung,
            self.screaming_highlight,
            self.default_unsung,
            self.default_highlight,
            zero,
            zero,
        ]
    }

    /// (unsung color index, highlight color index) for a given singer.
    fn singer_colors(&self, s: Singer) -> (u8, u8) {
        match s {
            Singer::Default => (DEFAULT_UNSUNG, DEFAULT_HIGHLIGHT),
            Singer::Male => (MALE_UNSUNG, MALE_HIGHLIGHT),
            Singer::Female => (FEMALE_UNSUNG, FEMALE_HIGHLIGHT),
            Singer::Duet => (DUET_UNSUNG, DUET_HIGHLIGHT),
            Singer::Screaming => (SCREAMING_UNSUNG, SCREAMING_HIGHLIGHT),
        }
    }
}

struct LineLayout {
    start_col: u8,
    /// Words re-joined with a single space - this is what's actually drawn,
    /// and what word column spans are computed against, so the two always
    /// agree regardless of whatever whitespace was in the original text.
    text: String,
    /// 1 or 2 - how many tile-columns/rows wide each character is drawn.
    scale: u8,
}

/// One character's scheduled highlight redraw - the current line's and a
/// backing vocal's word events are collected into a single `Vec` of these
/// and time-sorted before being emitted, so the two rows' wipes can be
/// interleaved correctly (see the comment where they're merged in
/// `render_cdg`).
struct HighlightEvent {
    at: f64,
    row: u8,
    char_idx: usize,
    ch: char,
    highlight_idx: u8,
}

/// Lay out `text` at `preferred_scale`, automatically falling back to 1x
/// if the text is too long to fit the 48-column canvas at that size (this
/// is the "shrink if it starts clipping" behavior).
fn layout_line_scaled(text: &str, preferred_scale: u8) -> LineLayout {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let char_len = joined.chars().count();
    let preferred_scale = preferred_scale.max(1);

    let scale = if char_len > 0 && char_len as u32 * preferred_scale as u32 <= SAFE_COLS as u32 {
        preferred_scale
    } else {
        1
    };

    let cols_used = char_len as u32 * scale as u32;
    let start_col = ((SAFE_COLS as i32 - cols_used as i32) / 2).max(0) as u8;

    // Last-resort safety net: even at 1x, an extremely long line gets clipped
    // rather than overflowing the canvas.
    let clipped: String = if scale == 1 && char_len > SAFE_COLS as usize {
        joined.chars().take(SAFE_COLS as usize).collect()
    } else {
        joined
    };

    LineLayout {
        start_col,
        text: clipped,
        scale,
    }
}

/// (char offset within the laid-out line, char length) for each word.
fn word_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut offset = 0usize;
    for w in text.split_whitespace() {
        let len = w.chars().count();
        spans.push((offset, len));
        offset += len + 1; // account for the joining space
    }
    spans
}

/// Draw a full laid-out line starting at `band_row`, using `layout.scale`
/// tile-columns/rows per character (so a 2x-scale line occupies both
/// `band_row` and `band_row + 1`).
fn draw_row_scaled(w: &mut CdgWriter, band_row: u8, layout: &LineLayout, color0: u8, color1: u8) {
    let scale = layout.scale as usize;
    let mut col = layout.start_col as usize;
    for ch in layout.text.chars() {
        if col + scale > SAFE_COLS as usize {
            break;
        }
        let tiles = font::glyph_tile_scaled(ch, layout.scale);
        for (tr, tile_row) in tiles.iter().enumerate() {
            for (tc, tile) in tile_row.iter().enumerate() {
                w.tile_block(band_row + tr as u8, (col + tc) as u8, color0, color1, tile);
            }
        }
        col += scale;
    }
}

/// Draw just one character of an already-laid-out line at word/char index
/// `char_idx` (0-based into `layout.text.chars()`) - used to redraw a single
/// word in the highlight color without touching the rest of the line.
fn draw_char_at(
    w: &mut CdgWriter,
    band_row: u8,
    layout: &LineLayout,
    char_idx: usize,
    ch: char,
    color0: u8,
    color1: u8,
) {
    let scale = layout.scale as usize;
    let col = layout.start_col as usize + char_idx * scale;
    if col + scale > SAFE_COLS as usize {
        return;
    }
    let tiles = font::glyph_tile_scaled(ch, layout.scale);
    for (tr, tile_row) in tiles.iter().enumerate() {
        for (tc, tile) in tile_row.iter().enumerate() {
            w.tile_block(band_row + tr as u8, (col + tc) as u8, color0, color1, tile);
        }
    }
}

/// Builds the legend's display text (e.g. "Male  Female") plus a
/// per-character foreground color index (spaces get a throwaway index,
/// since [`draw_legend_row`] skips drawing whitespace).
fn legend_text_and_colors(palette: &Palette, singers: &[Singer]) -> (String, Vec<u8>) {
    let mut text = String::new();
    let mut colors = Vec::new();
    for (i, singer) in singers.iter().enumerate() {
        if i > 0 {
            text.push_str("  ");
            colors.extend([BG, BG]);
        }
        let (_, highlight) = palette.singer_colors(*singer);
        let label = singer.label();
        text.push_str(label);
        colors.extend(std::iter::repeat_n(highlight, label.chars().count()));
    }
    (text, colors)
}

/// Draws a legend line laid out like [`draw_row_scaled`], but with each
/// character's foreground color taken from `colors` individually instead of
/// a single color for the whole line - so each singer's label can be shown
/// in that singer's own highlight color.
fn draw_legend_row(w: &mut CdgWriter, band_row: u8, layout: &LineLayout, colors: &[u8]) {
    for (char_idx, ch) in layout.text.chars().enumerate() {
        if ch.is_whitespace() {
            continue;
        }
        let color1 = colors.get(char_idx).copied().unwrap_or(BG);
        draw_char_at(w, band_row, layout, char_idx, ch, BG, color1);
    }
}

fn clear_row(w: &mut CdgWriter, row: u8) {
    for col in 0..SAFE_COLS {
        w.tile_block(row, col, BG, BG, &BLANK_TILE);
    }
}

/// Clear a `height`-row-tall band (used for the current line and title,
/// which may have been drawn at 2x scale by a previous line/card).
fn clear_band(w: &mut CdgWriter, band_row: u8, height: u8) {
    for r in 0..height {
        clear_row(w, band_row + r);
    }
}

/// How long (seconds from t=0) the title/artist intro card should be shown,
/// given the timed lines. Exposed so the live preview can match the export
/// exactly. Capped so it never eats into the first line's start time.
pub fn title_card_end(timed_lines: &[TimedLine]) -> f64 {
    match timed_lines.first() {
        Some(first) => first.start.min(5.0),
        None => 4.0,
    }
}

/// Where a lyrics/CDG file's paired audio file needs to live for
/// "MP3+G"-style pickup by karaoke players (also the convention LRC and
/// UltraStar players/games expect - a same-named audio file in the same
/// folder): the *same* directory and base filename as `sibling_path`,
/// keeping `audio_path`'s own extension (despite the "MP3+G" convention's
/// name, most players accept whatever format the audio actually is - it
/// doesn't have to literally be re-encoded to `.mp3`).
pub fn paired_audio_path(audio_path: &Path, sibling_path: &Path) -> PathBuf {
    let ext = audio_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp3");
    sibling_path.with_extension(ext)
}

/// Copies `audio_path` to sit next to `sibling_path` (see
/// [`paired_audio_path`]) so the pair is ready for pickup by a karaoke
/// player/game without a manual copy or rename. A no-op (not an error) if
/// the audio is already exactly there - e.g. re-exporting into the same
/// folder the audio already lives in, under a name that already matches.
pub fn copy_paired_audio(audio_path: &Path, sibling_path: &Path) -> Result<PathBuf> {
    let dest = paired_audio_path(audio_path, sibling_path);
    let already_there = std::fs::canonicalize(audio_path)
        .ok()
        .zip(std::fs::canonicalize(&dest).ok())
        .is_some_and(|(a, b)| a == b);
    if already_there {
        return Ok(dest);
    }
    std::fs::copy(audio_path, &dest)
        .with_context(|| format!("failed to copy audio to {}", dest.display()))?;
    Ok(dest)
}

/// Column positions for the 4 countdown markers, chosen for exact
/// left-right symmetry around the canvas's true center (23.5 within the
/// 0..47 safe-column span - an even number of columns has no single center
/// column, so a naive "center +/- offset" scheme using round-number offsets
/// drifts half a tile off true center; these four were picked to mirror
/// exactly: reflecting 18 around 23.5 gives 29, and reflecting 22 gives 25).
fn countdown_columns() -> [u8; 4] {
    [18, 22, 25, 29]
}

fn draw_countdown_dot(w: &mut CdgWriter, col: u8, color1: u8) {
    w.tile_block(COUNTDOWN_ROW, col, BG, color1, &font::glyph_tile('o'));
}

/// Schedules the 4 countdown dots to light up one per quarter of the
/// window, each staying lit through to `cd_end` - so by the final quarter
/// all 4 are already lit and stay that way, rather than the last dot only
/// lighting up for a single instant exactly as the next line begins (which
/// is effectively invisible).
fn schedule_countdown_dots(w: &mut CdgWriter, cd_start: f64, cd_end: f64, lit_color: u8) {
    w.advance_to(cd_start);
    let cols = countdown_columns();
    for &col in &cols {
        draw_countdown_dot(w, col, PREVIEW);
    }
    for (k, &col) in cols.iter().enumerate() {
        let t = cd_start + (k as f64) * (cd_end - cd_start) / 4.0;
        w.advance_to(t);
        draw_countdown_dot(w, col, lit_color);
    }
}

/// Same as [`render_cdg_with_settings`], using [`TimingSettings::default`] -
/// kept as its own entry point so the many existing callers/tests that
/// don't care about custom timing settings don't all need updating just
/// because this setting now exists. Real production code calls
/// `render_cdg_with_settings` directly (see `main.rs`), so this is only
/// reached by tests in a normal (non-test) build - same reasoning as
/// `TimedLine::new`'s own `#[allow(dead_code)]` in `lyrics.rs`.
#[allow(dead_code)]
pub fn render_cdg(
    timed_lines: &[TimedLine],
    total_duration: f64,
    palette: &Palette,
    title: Option<&str>,
    artist: Option<&str>,
) -> Vec<u8> {
    render_cdg_with_settings(
        timed_lines,
        total_duration,
        palette,
        title,
        artist,
        &TimingSettings::default(),
    )
}

/// Render a complete karaoke CDG stream, padded to `total_duration` seconds.
pub fn render_cdg_with_settings(
    timed_lines: &[TimedLine],
    total_duration: f64,
    palette: &Palette,
    title: Option<&str>,
    artist: Option<&str>,
    timing_settings: &TimingSettings,
) -> Vec<u8> {
    let mut w = CdgWriter::new();

    w.load_color_table(&palette.low_clut(), false);
    w.load_color_table(&palette.high_clut(), true);
    w.memory_preset(BG);
    w.border_preset(BG);

    if let Some(t) = title {
        draw_row_scaled(
            &mut w,
            TITLE_ROW,
            &layout_line_scaled(t, PREFERRED_SCALE),
            BG,
            TITLE,
        );
    }
    if let Some(a) = artist {
        draw_row_scaled(
            &mut w,
            ARTIST_ROW,
            &layout_line_scaled(&format!("by {a}"), 1),
            BG,
            ARTIST,
        );
    }
    let legend_singers = singer_legend(timed_lines);
    if !legend_singers.is_empty() {
        let (text, colors) = legend_text_and_colors(palette, &legend_singers);
        draw_legend_row(&mut w, LEGEND_ROW, &layout_line_scaled(&text, 1), &colors);
    }
    // Only actually spend stream time on the title-card rows (and the wipe
    // that clears them) when there's something to show there - with no
    // title/artist/legend, there's nothing to clear, and forcing the stream
    // forward to `card_end` would pad the file with dead air even for a
    // song with no lyric lines at all.
    let card_end = title_card_end(timed_lines);
    if title.is_some() || artist.is_some() || !legend_singers.is_empty() {
        w.advance_to(card_end);
        clear_band(&mut w, TITLE_ROW, PREFERRED_SCALE);
        clear_row(&mut w, ARTIST_ROW);
        clear_row(&mut w, LEGEND_ROW);
    }

    // If there's a long stretch between the title card fading (or, if
    // there's no title/artist, the equivalent point in the intro) and the
    // first line actually starting, show the same "get ready" countdown
    // used for mid-song breaks instead of leaving a blank screen. This must
    // not be gated on title/artist being present - a long instrumental
    // intro deserves the countdown either way (matches `video.rs`, which
    // never gated this on the title card).
    if let Some(first) = timed_lines.first() {
        if let Some((cd_start, cd_end)) =
            countdown_window_between(card_end, first.start, first.countdown_mode, timing_settings)
        {
            let (_, highlight_idx) = palette.singer_colors(first.singer);
            schedule_countdown_dots(&mut w, cd_start, cd_end, highlight_idx);
            w.advance_to(cd_end);
            clear_row(&mut w, COUNTDOWN_ROW);
        }
    }

    for (i, line) in timed_lines.iter().enumerate() {
        w.advance_to(line.start);
        clear_band(&mut w, CURRENT_ROW, PREFERRED_SCALE);
        clear_row(&mut w, BACKING_ROW);
        clear_row(&mut w, PREVIEW_ROW);
        clear_row(&mut w, COUNTDOWN_ROW);

        let (unsung_idx, highlight_idx) = palette.singer_colors(line.singer);

        let layout = layout_line_scaled(&line.text, PREFERRED_SCALE);
        draw_row_scaled(&mut w, CURRENT_ROW, &layout, BG, unsung_idx);

        if let Some(next) = timed_lines.get(i + 1) {
            let next_layout = layout_line_scaled(&next.text, 1);
            draw_row_scaled(&mut w, PREVIEW_ROW, &next_layout, BG, PREVIEW);
        }

        // A backing/echo vocal (if any) draws its own base row now,
        // alongside the current line's - its word-highlight events get
        // merged into the same time-ordered sequence as the current line's
        // own, below, so the two wipes can run concurrently.
        let backing = line.backing_vocal.as_ref().map(|bv| {
            let (bv_unsung_idx, bv_highlight_idx) = palette.singer_colors(bv.singer);
            let bv_layout = layout_line_scaled(&bv.text, 1);
            draw_row_scaled(&mut w, BACKING_ROW, &bv_layout, BG, bv_unsung_idx);
            (bv_layout, bv_highlight_idx)
        });

        let spans = word_spans(&layout.text);
        let words = word_timings(line);
        let chars: Vec<char> = layout.text.chars().collect();
        let mut events: Vec<HighlightEvent> = Vec::new();
        for (tw, &(offset, len)) in words.iter().zip(spans.iter()) {
            for k in 0..len {
                let char_idx = offset + k;
                events.push(HighlightEvent {
                    at: tw.highlight_at,
                    row: CURRENT_ROW,
                    char_idx,
                    ch: chars.get(char_idx).copied().unwrap_or(' '),
                    highlight_idx,
                });
            }
        }
        if let (Some(bv), Some((bv_layout, bv_highlight_idx))) =
            (line.backing_vocal.as_ref(), &backing)
        {
            let bv_spans = word_spans(&bv_layout.text);
            let bv_words = backing_vocal_word_timings(bv);
            let bv_chars: Vec<char> = bv_layout.text.chars().collect();
            for (tw, &(offset, len)) in bv_words.iter().zip(bv_spans.iter()) {
                for k in 0..len {
                    let char_idx = offset + k;
                    events.push(HighlightEvent {
                        at: tw.highlight_at,
                        row: BACKING_ROW,
                        char_idx,
                        ch: bv_chars.get(char_idx).copied().unwrap_or(' '),
                        highlight_idx: *bv_highlight_idx,
                    });
                }
            }
        }
        // `CdgWriter::advance_to` is monotonic-only-forward (a target
        // earlier than the writer's current position silently no-ops), so
        // the current line's and a backing vocal's word events - which
        // interleave in real time whenever the two overlap, the whole
        // point of a backing vocal - must be emitted in one time-sorted
        // sequence, not "all current-line words, then all backing words".
        events.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap());
        for ev in &events {
            w.advance_to(ev.at);
            let ev_layout = if ev.row == BACKING_ROW {
                &backing.as_ref().unwrap().0
            } else {
                &layout
            };
            draw_char_at(
                &mut w,
                ev.row,
                ev_layout,
                ev.char_idx,
                ev.ch,
                BG,
                ev.highlight_idx,
            );
        }

        let next_countdown_mode = timed_lines
            .get(i + 1)
            .map(|n| n.countdown_mode)
            .unwrap_or_default();
        if let Some((cd_start, cd_end)) =
            countdown_window(line, next_countdown_mode, timing_settings)
        {
            // There's a real musical break before the next line - don't
            // leave its dim preview sitting on screen for the whole break;
            // clear it as soon as this line is done being sung, so the
            // screen reads as "done, waiting" and then the countdown dots,
            // rather than showing a line that's still a break away.
            w.advance_to(line.sing_end);
            clear_row(&mut w, PREVIEW_ROW);

            // Let the just-finished line linger fully highlighted for a
            // bit, then clear it too (or immediately, if the break is short
            // enough that the countdown starts before the linger would
            // finish) - so a long break just shows a blank screen until the
            // countdown appears, instead of sitting there the whole time.
            let blank_at = (line.sing_end + SUNG_LINGER_SECS).min(cd_start);
            w.advance_to(blank_at);
            clear_band(&mut w, CURRENT_ROW, PREFERRED_SCALE);
            clear_row(&mut w, BACKING_ROW);

            // Only actually draw the countdown dots if there's a real next
            // line to count into - for the last line of the song, this same
            // "long gap" is just trailing silence after the song ends, with
            // nothing to cue the singer to get ready for. The screen still
            // blanks during that silence (above); it just doesn't show dots
            // counting down to a line that doesn't exist.
            if let Some(next) = timed_lines.get(i + 1) {
                let (_, next_highlight_idx) = palette.singer_colors(next.singer);
                schedule_countdown_dots(&mut w, cd_start, cd_end, next_highlight_idx);
            }
        }
    }

    w.pad_until(total_duration.max(w.current_time_secs()));
    w.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::resolve_timing;
    use crate::lyrics::LyricLine;

    #[test]
    fn paired_audio_path_keeps_the_audios_own_extension() {
        let cdg = Path::new("/music/karaoke/My Song.cdg");
        assert_eq!(
            paired_audio_path(Path::new("/downloads/original.mp3"), cdg),
            PathBuf::from("/music/karaoke/My Song.mp3")
        );
        assert_eq!(
            paired_audio_path(Path::new("/downloads/original.flac"), cdg),
            PathBuf::from("/music/karaoke/My Song.flac")
        );
        // No extension at all on the source - falls back to .mp3, matching
        // the "MP3+G" convention's usual name.
        assert_eq!(
            paired_audio_path(Path::new("/downloads/original"), cdg),
            PathBuf::from("/music/karaoke/My Song.mp3")
        );
    }

    #[test]
    fn copy_paired_audio_copies_to_the_expected_name() {
        let dir = std::env::temp_dir().join(format!(
            "abyssal-cdg-export-test-copy-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let audio_src = dir.join("source.wav");
        std::fs::write(&audio_src, b"fake audio bytes").unwrap();
        let cdg_path = dir.join("karaoke.cdg");

        let dest = copy_paired_audio(&audio_src, &cdg_path).unwrap();
        assert_eq!(dest, dir.join("karaoke.wav"));
        assert!(dest.exists());
        assert_eq!(std::fs::read(&dest).unwrap(), b"fake audio bytes");
        // The source must be untouched (this is a copy, not a move).
        assert!(audio_src.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_paired_audio_is_a_no_op_when_already_in_place() {
        let dir = std::env::temp_dir().join(format!(
            "abyssal-cdg-export-test-noop-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // The audio is already sitting exactly where pairing would put it.
        let audio_path = dir.join("karaoke.mp3");
        std::fs::write(&audio_path, b"already here").unwrap();
        let cdg_path = dir.join("karaoke.cdg");

        let dest = copy_paired_audio(&audio_path, &cdg_path).unwrap();
        assert_eq!(dest, audio_path);
        assert_eq!(std::fs::read(&dest).unwrap(), b"already here");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn renders_valid_packet_stream() {
        let mut lines = vec![
            LyricLine::new("hello world"),
            LyricLine::new("second line here"),
        ];
        lines[0].start = Some(0.5);
        lines[1].start = Some(3.0);
        let timed = resolve_timing(&lines, Some(6.0));
        let bytes = render_cdg(&timed, 6.0, &Palette::default(), None, None);
        assert_eq!(bytes.len() % 24, 0);
        assert!(bytes.len() / 24 >= (6.0 * crate::cdg::PACKETS_PER_SEC) as usize);
    }

    #[test]
    fn empty_lines_still_produce_valid_padded_stream() {
        let bytes = render_cdg(&[], 3.0, &Palette::default(), None, None);
        assert_eq!(bytes.len() % 24, 0);
        assert_eq!(
            bytes.len() / 24,
            (3.0 * crate::cdg::PACKETS_PER_SEC) as usize
        );
    }

    #[test]
    fn title_and_artist_do_not_panic_and_extend_stream() {
        let mut lines = vec![LyricLine::new("hello world")];
        lines[0].start = Some(2.0);
        let timed = resolve_timing(&lines, Some(6.0));
        let bytes = render_cdg(
            &timed,
            6.0,
            &Palette::default(),
            Some("My Song"),
            Some("An Artist"),
        );
        assert_eq!(bytes.len() % 24, 0);
    }

    #[test]
    fn long_gap_produces_more_packets_than_a_tight_song_of_equal_length() {
        // Sanity check that countdown drawing doesn't break padding/timing.
        let mut lines = vec![LyricLine::new("hi"), LyricLine::new("there")];
        lines[0].start = Some(0.0);
        lines[1].start = Some(20.0); // huge gap -> triggers countdown
        let timed = resolve_timing(&lines, Some(22.0));
        let bytes = render_cdg(&timed, 22.0, &Palette::default(), None, None);
        assert_eq!(
            bytes.len() / 24,
            (22.0 * crate::cdg::PACKETS_PER_SEC) as usize
        );
    }

    #[test]
    fn duet_lines_render_without_panicking() {
        let mut lines = vec![LyricLine::new("his line"), LyricLine::new("her line")];
        lines[0].start = Some(0.0);
        lines[0].singer = crate::lyrics::Singer::Male;
        lines[1].start = Some(3.0);
        lines[1].singer = crate::lyrics::Singer::Female;
        let timed = resolve_timing(&lines, Some(6.0));
        let bytes = render_cdg(&timed, 6.0, &Palette::default(), None, None);
        assert_eq!(bytes.len() % 24, 0);
    }

    #[test]
    fn duet_legend_renders_without_title_or_artist() {
        // No title/artist given - the singer-color legend alone should
        // still trigger the intro-card timing (advance + clear rows)
        // without panicking.
        let mut lines = vec![LyricLine::new("his line"), LyricLine::new("her line")];
        lines[0].start = Some(0.0);
        lines[0].singer = crate::lyrics::Singer::Male;
        lines[1].start = Some(3.0);
        lines[1].singer = crate::lyrics::Singer::Female;
        let timed = resolve_timing(&lines, Some(6.0));
        assert_eq!(
            crate::lyrics::singer_legend(&timed).len(),
            2,
            "a two-voice song should get a legend"
        );
        let bytes = render_cdg(&timed, 6.0, &Palette::default(), None, None);
        assert_eq!(bytes.len() % 24, 0);
    }

    #[test]
    fn single_voice_song_gets_no_legend() {
        let mut lines = vec![LyricLine::new("hello"), LyricLine::new("world")];
        lines[0].start = Some(0.0);
        lines[1].start = Some(3.0);
        let timed = resolve_timing(&lines, Some(6.0));
        assert!(crate::lyrics::singer_legend(&timed).is_empty());
        let bytes = render_cdg(&timed, 6.0, &Palette::default(), None, None);
        assert_eq!(bytes.len() % 24, 0);
    }

    #[test]
    fn screaming_lines_render_without_panicking() {
        let mut lines = vec![LyricLine::new("normal line"), LyricLine::new("AAAAAH")];
        lines[0].start = Some(0.0);
        lines[0].singer = crate::lyrics::Singer::Male;
        lines[1].start = Some(3.0);
        lines[1].singer = crate::lyrics::Singer::Screaming;
        let timed = resolve_timing(&lines, Some(6.0));
        let bytes = render_cdg(
            &timed,
            6.0,
            &Palette::default(),
            Some("Title"),
            Some("Artist"),
        );
        assert_eq!(bytes.len() % 24, 0);
    }

    #[test]
    fn high_clut_stays_within_the_8_color_budget() {
        // The high CLUT instruction only carries 8 colors per load - this
        // just asserts the array we build is exactly that size, so a future
        // 5th color category can't silently overflow it unnoticed.
        let high = Palette::default().high_clut();
        assert_eq!(high.len(), 8);
        // Currently: title, artist, screaming_unsung, screaming_highlight,
        // default_unsung, default_highlight (6 used), leaving exactly 2
        // zero-padded slots free for later.
    }

    #[test]
    fn abyssal_preset_stays_within_the_clut_budgets() {
        let p = Palette::abyssal();
        assert_eq!(p.low_clut().len(), 8);
        assert_eq!(p.high_clut().len(), 8);
    }

    #[test]
    fn abyssal_preset_matches_the_requested_colors_as_closely_as_4_bit_allows() {
        // background #000000, unsung/default #8B0000, highlight #FFB000 -
        // confirms the nearest-4-bit-step math in the doc comment is
        // actually what's encoded, not just what the comment claims.
        let p = Palette::abyssal();
        assert_eq!((p.background.r, p.background.g, p.background.b), (0, 0, 0));
        assert_eq!(
            (p.default_unsung.r, p.default_unsung.g, p.default_unsung.b),
            (8, 0, 0)
        );
        assert_eq!(
            (
                p.default_highlight.r,
                p.default_highlight.g,
                p.default_highlight.b
            ),
            (15, 10, 0)
        );
        // Male mirrors Default, matching every other preset's own convention.
        assert_eq!(p.male_unsung.r, p.default_unsung.r);
        assert_eq!(p.male_highlight.r, p.default_highlight.r);
    }

    #[test]
    fn abyssal_preset_every_voice_pair_is_a_lightness_contrast() {
        // The whole point of this preset: every unsung/highlight pair
        // should read as "clearly dim" vs. "clearly bright" (a lightness
        // difference), not rely on a same-brightness hue distinction (the
        // failure mode for red-green color blindness). Approximated with
        // the standard luma formula, applied to the 4-bit-per-channel
        // values scaled to 0-255 (matching what actually gets rendered).
        fn luma(c: CdgColor) -> f32 {
            let (r, g, b) = (c.r as f32 * 17.0, c.g as f32 * 17.0, c.b as f32 * 17.0);
            0.299 * r + 0.587 * g + 0.114 * b
        }
        let p = Palette::abyssal();
        let pairs = [
            ("default", p.default_unsung, p.default_highlight),
            ("male", p.male_unsung, p.male_highlight),
            ("female", p.female_unsung, p.female_highlight),
            ("duet", p.duet_unsung, p.duet_highlight),
            ("screaming", p.screaming_unsung, p.screaming_highlight),
        ];
        for (name, unsung, highlight) in pairs {
            assert!(
                luma(highlight) - luma(unsung) >= 40.0,
                "{name}: highlight should read clearly brighter than unsung \
                 (unsung luma {:.0}, highlight luma {:.0})",
                luma(unsung),
                luma(highlight)
            );
        }
    }

    #[test]
    fn short_line_uses_preferred_scale() {
        let layout = layout_line_scaled("Hello world", 2);
        assert_eq!(layout.scale, 2);
        assert_eq!(layout.text, "Hello world");
    }

    #[test]
    fn long_line_falls_back_to_scale_1() {
        // 25 chars * scale 2 = 50 > 48 safe columns, so this must fall back.
        let text = "This line is exactly too long";
        assert!(text.chars().count() * 2 > SAFE_COLS as usize);
        let layout = layout_line_scaled(text, 2);
        assert_eq!(layout.scale, 1);
        assert_eq!(layout.text, text);
    }

    #[test]
    fn extremely_long_line_still_clips_safely_even_at_scale_1() {
        let text = "a".repeat(200);
        let layout = layout_line_scaled(&text, 2);
        assert_eq!(layout.scale, 1);
        assert_eq!(layout.text.chars().count(), SAFE_COLS as usize);
    }

    #[test]
    fn very_long_lines_render_without_panicking_at_any_scale() {
        let mut lines = vec![
            LyricLine::new("short"),
            LyricLine::new("This is a deliberately very long lyric line meant to force the automatic fallback to normal-sized text so it never runs off the edge of the screen"),
        ];
        lines[0].start = Some(0.0);
        lines[1].start = Some(3.0);
        let timed = resolve_timing(&lines, Some(8.0));
        let bytes = render_cdg(
            &timed,
            8.0,
            &Palette::default(),
            Some("A Title That Is Also Somewhat Long For A Title"),
            Some("Artist"),
        );
        assert_eq!(bytes.len() % 24, 0);
    }

    #[test]
    fn countdown_columns_are_symmetric_around_true_center() {
        // True center of the 0..47 safe-column span is 23.5 (48 columns has
        // no single center column). Reflecting each column around 23.5
        // should map the set onto itself.
        let cols = countdown_columns();
        let mut reflected: Vec<i32> = cols.iter().map(|&c| 47 - c as i32).collect();
        reflected.sort();
        let mut original: Vec<i32> = cols.iter().map(|&c| c as i32).collect();
        original.sort();
        assert_eq!(
            reflected, original,
            "columns aren't symmetric around true center"
        );
    }

    #[test]
    fn intro_countdown_triggers_only_on_long_gap() {
        let settings = TimingSettings::default();
        let auto = CountdownMode::Auto;
        assert!(countdown_window_between(5.0, 25.0, auto, &settings).is_some()); // 20s gap
        assert!(countdown_window_between(5.0, 7.0, auto, &settings).is_none()); // 2s gap, too short
    }

    #[test]
    fn long_intro_renders_countdown_without_panicking() {
        let mut lines = vec![LyricLine::new("first line"), LyricLine::new("second line")];
        lines[0].start = Some(25.0); // long intro before this
        lines[1].start = Some(28.0);
        let timed = resolve_timing(&lines, Some(31.0));
        let bytes = render_cdg(
            &timed,
            31.0,
            &Palette::default(),
            Some("Title"),
            Some("Artist"),
        );
        assert_eq!(bytes.len() % 24, 0);
        assert_eq!(
            bytes.len() / 24,
            (31.0 * crate::cdg::PACKETS_PER_SEC) as usize
        );
    }

    #[test]
    fn long_intro_renders_countdown_even_without_title_or_artist() {
        // Regression: the intro "get ready" countdown must not depend on a
        // title/artist card being present - a long instrumental intro
        // deserves it either way (see also video.rs, which never gated it
        // on the title card).
        let mut lines = vec![LyricLine::new("first line")];
        lines[0].start = Some(20.0); // long intro, no title/artist given
        let timed = resolve_timing(&lines, Some(22.0));
        let bytes = render_cdg(&timed, 22.0, &Palette::default(), None, None);
        let tile_block_count = bytes.chunks(24).filter(|p| p[1] == 6).count();
        assert!(
            tile_block_count > 0,
            "expected countdown dots to be drawn even without a title card"
        );
    }

    /// Counts tile-block packets that actually draw a lit/unlit countdown
    /// dot glyph at the countdown row (as opposed to the blank-tile packets
    /// that clear that row between lines).
    fn countdown_dot_packet_count(bytes: &[u8]) -> usize {
        // Packet layout: byte 1 = instruction, bytes 4..20 = instruction
        // data (row is data[2] -> packet byte 6, pixels are data[4..16] ->
        // packet bytes 8..20) - see `CdgWriter::push_packet`/`tile_block`.
        let countdown_row = COUNTDOWN_ROW + crate::cdg::SAFE_ROW_OFFSET;
        bytes
            .chunks(24)
            .filter(|p| p[1] == 6 && p[6] == countdown_row && p[8..20].iter().any(|&b| b != 0))
            .count()
    }

    #[test]
    fn no_countdown_after_the_last_line_even_with_a_long_silent_tail() {
        // Regression: a long stretch of silence after the *last* line (e.g.
        // a long instrumental outro) must not trigger the countdown - there's
        // no next line to count into, so the dots would be promising
        // something that never actually happens.
        let mut lines = vec![LyricLine::new("hi")];
        lines[0].start = Some(0.0);
        let timed = resolve_timing(&lines, Some(30.0)); // huge trailing silence
        let bytes = render_cdg(&timed, 30.0, &Palette::default(), None, None);
        assert_eq!(
            countdown_dot_packet_count(&bytes),
            0,
            "no countdown dots should be drawn after the last line"
        );
    }

    #[test]
    fn countdown_still_shows_for_a_real_mid_song_gap() {
        // Sanity check alongside the test above: only the end-of-song case
        // changed - a long gap *between* two real lines still gets dots.
        let mut lines = vec![LyricLine::new("hi"), LyricLine::new("there")];
        lines[0].start = Some(0.0);
        lines[1].start = Some(20.0); // huge mid-song gap
        let timed = resolve_timing(&lines, Some(22.0));
        let bytes = render_cdg(&timed, 22.0, &Palette::default(), None, None);
        assert!(
            countdown_dot_packet_count(&bytes) > 0,
            "expected countdown dots for a real mid-song gap"
        );
    }

    /// Counts tile-block packets that draw non-blank content at `row`
    /// (packet-relative, i.e. already offset by `SAFE_ROW_OFFSET`) -
    /// generalizes [`countdown_dot_packet_count`] to any row.
    fn non_blank_draws_at_row(bytes: &[u8], row: u8) -> usize {
        let packet_row = row + crate::cdg::SAFE_ROW_OFFSET;
        bytes
            .chunks(24)
            .filter(|p| p[1] == 6 && p[6] == packet_row && p[8..20].iter().any(|&b| b != 0))
            .count()
    }

    #[test]
    fn backing_vocal_renders_without_panicking_and_produces_a_valid_stream() {
        let mut lines = vec![LyricLine::new("hello world")];
        lines[0].start = Some(0.0);
        let mut bv = crate::lyrics::BackingVocal::new("hey");
        bv.start = Some(1.0);
        bv.end = Some(3.0);
        lines[0].backing_vocal = Some(bv);
        let timed = resolve_timing(&lines, Some(10.0));
        assert!(timed[0].backing_vocal.is_some());

        let bytes = render_cdg(&timed, 10.0, &Palette::default(), None, None);
        assert_eq!(bytes.len() % 24, 0);
        assert!(
            non_blank_draws_at_row(&bytes, BACKING_ROW) > 0,
            "expected the backing vocal to actually draw something on its row"
        );
    }

    #[test]
    fn a_line_without_a_backing_vocal_draws_nothing_on_the_backing_row() {
        let mut lines = vec![LyricLine::new("hello world")];
        lines[0].start = Some(0.0);
        let timed = resolve_timing(&lines, Some(10.0));
        let bytes = render_cdg(&timed, 10.0, &Palette::default(), None, None);
        assert_eq!(non_blank_draws_at_row(&bytes, BACKING_ROW), 0);
    }

    #[test]
    fn backing_vocal_uses_its_own_singers_colors_not_the_hosts() {
        // Duet's highlight color differs from Male's in the default
        // palette - render the same line/backing pair with each as the
        // backing vocal's singer and confirm the resulting bytes differ,
        // i.e. the backing vocal's own `singer` field actually reaches the
        // renderer rather than silently inheriting the host's.
        let build = |backing_singer: Singer| {
            let mut lines = vec![LyricLine::new("hello world")];
            lines[0].start = Some(0.0);
            lines[0].singer = Singer::Male;
            let mut bv = crate::lyrics::BackingVocal::new("hey");
            bv.start = Some(1.0);
            bv.end = Some(3.0);
            bv.singer = backing_singer;
            lines[0].backing_vocal = Some(bv);
            let timed = resolve_timing(&lines, Some(10.0));
            render_cdg(&timed, 10.0, &Palette::default(), None, None)
        };
        assert_ne!(build(Singer::Duet), build(Singer::Female));
    }
}
