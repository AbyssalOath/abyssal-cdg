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
//! backing-track line always renders at 1x - a smaller, visually secondary
//! row.
//!
//! Color palette: the CDG format gives us 16 color slots, and all 16 are
//! used: 8 for background/voice text colors (loaded via the low CLUT,
//! completely full: background + 3 voices x 2 colors + preview), and 8
//! more for the title/artist card plus the "screaming", "default" and
//! "backing" voice color pairs (loaded via the high CLUT, also full). There
//! is no room left for another voice color pair.

use crate::cdg::{CdgColor, CdgWriter, TilePixels, BLANK_TILE, SAFE_COLS};
use crate::font;
#[cfg(test)]
use crate::lyrics::CountdownMode;
use crate::lyrics::{
    backing_display_windows, countdown_window, countdown_window_between, effective_singer_label,
    singer_legend, word_timings, Singer, TimedLine, TimingSettings, SUNG_LINGER_SECS,
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
/// The backing track (see [`crate::lyrics::LyricLine::is_backing`])
/// renders here, directly beneath the current line's own band, always at
/// 1x scale - a smaller row for what's meant to read as a secondary voice,
/// with its own independent word-wipe running concurrently with whatever
/// main line is playing. This row is owned entirely by the backing track's
/// own schedule (see [`BackingLane`]), independent of the main lines'
/// loop, since a backing line can start/end anywhere relative to them.
/// Free during normal singing (rows 8-9, 11-12, and 14-15 are all
/// otherwise unused outside the title card), so this doesn't crowd
/// anything else out.
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
// High CLUT (palette indices 8-15, loaded separately; all 8 used):
const TITLE: u8 = 8;
const ARTIST: u8 = 9;
const SCREAMING_UNSUNG: u8 = 10;
const SCREAMING_HIGHLIGHT: u8 = 11;
const DEFAULT_UNSUNG: u8 = 12;
const DEFAULT_HIGHLIGHT: u8 = 13;
const BACKING_UNSUNG: u8 = 14;
const BACKING_HIGHLIGHT: u8 = 15;

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
    /// Color pair for [`Singer::Backing`] - the default voice of a
    /// backing-track line.
    pub backing_unsung: CdgColor,
    pub backing_highlight: CdgColor,
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
            // Duet sings in sky blue (not orange): orange vs Male's yellow
            // is near-indistinguishable with red-green color blindness, the
            // most common kind; blue vs yellow reads for everyone.
            duet_unsung: CdgColor::new(10, 15, 10),
            duet_highlight: CdgColor::new(4, 11, 15),
            preview: CdgColor::new(6, 6, 9),
            title: CdgColor::new(15, 15, 15),
            artist: CdgColor::new(9, 9, 12),
            // Pale pink upcoming (the old dark red, 9/2/1, was ~2.5:1 against
            // the background - hard to read) wiping to bright red.
            screaming_unsung: CdgColor::new(15, 11, 10),
            screaming_highlight: CdgColor::new(15, 3, 0),
            backing_unsung: CdgColor::new(8, 11, 13),
            backing_highlight: CdgColor::new(0, 15, 15),
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
            screaming_unsung: CdgColor::new(15, 10, 10),
            screaming_highlight: CdgColor::new(15, 0, 0),
            backing_unsung: CdgColor::new(6, 8, 9),
            backing_highlight: CdgColor::new(0, 15, 8),
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
            screaming_unsung: CdgColor::new(14, 9, 10),
            screaming_highlight: CdgColor::new(15, 2, 3),
            backing_unsung: CdgColor::new(8, 7, 11),
            backing_highlight: CdgColor::new(13, 8, 15),
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
            screaming_unsung: CdgColor::new(10, 10, 15),
            screaming_highlight: CdgColor::new(9, 0, 15),
            backing_unsung: CdgColor::new(6, 10, 8),
            backing_highlight: CdgColor::new(12, 15, 15),
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
            // Blood red (~#BB0000) -> warm gold (~#FFB000). Every upcoming
            // color in this preset is scaled up (same hue) just enough to
            // reach ~3:1 contrast on the black background - still dark and
            // moody, but readable on a TV across a room.
            default_unsung: CdgColor::new(11, 0, 0),
            default_highlight: CdgColor::new(15, 10, 0),
            male_unsung: CdgColor::new(11, 0, 0),
            male_highlight: CdgColor::new(15, 10, 0),
            // Violet -> bright magenta - same dark/bright split as the
            // default pair, on a blue-red axis with no green component at
            // all, so it can't be confused with the red/gold pair even
            // under red-green color blindness.
            female_unsung: CdgColor::new(7, 4, 11),
            // (A touch brighter than the original (15, 3, 10) so it stays
            // clearly brighter than the brightened violet above.)
            female_highlight: CdgColor::new(15, 5, 12),
            // Teal -> bright cyan - a blue-green pair (green only ever
            // shows up paired against *more blue-green*, never against red).
            duet_unsung: CdgColor::new(3, 6, 6),
            duet_highlight: CdgColor::new(2, 15, 14),
            // Dark brick -> stark white - the largest possible
            // lightness jump, and no hue reliance whatsoever.
            screaming_unsung: CdgColor::new(11, 2, 2),
            screaming_highlight: CdgColor::new(15, 15, 15),
            // Olive-brown -> pale straw - another dark/bright split,
            // on a yellow axis distinct from the gold default highlight by
            // being much paler.
            backing_unsung: CdgColor::new(7, 5, 1),
            backing_highlight: CdgColor::new(15, 15, 9),
            // Dim red-grey - still the faintest text on screen, matching
            // every other preset's "preview" being a dim tone near its own
            // background, but legible (~1.8:1).
            preview: CdgColor::new(6, 2, 2),
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
        [
            self.title,
            self.artist,
            self.screaming_unsung,
            self.screaming_highlight,
            self.default_unsung,
            self.default_highlight,
            self.backing_unsung,
            self.backing_highlight,
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
            Singer::Backing => (BACKING_UNSUNG, BACKING_HIGHLIGHT),
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

/// One character's scheduled highlight redraw on the current line's row.
struct HighlightEvent {
    at: f64,
    char_idx: usize,
    ch: char,
    highlight_idx: u8,
}

/// Something the backing lane (see [`BACKING_ROW`]) needs to do at a given
/// time - see [`BackingLane`].
enum BackingOp {
    /// Draw backing line `k`'s whole text in its unsung color.
    Show(usize),
    /// Highlight one character of backing line `k`.
    Highlight {
        k: usize,
        char_idx: usize,
        ch: char,
        color: u8,
    },
    Clear,
}

/// The backing track's own pre-computed draw schedule. `CdgWriter::advance_to`
/// is monotonic-only-forward (a target earlier than the writer's current
/// position silently no-ops), and backing lines start/end independently of
/// the main lines - so rather than trying to fit them into the main loop,
/// every backing-lane operation is computed up front and time-sorted here,
/// and *every* forward step the main loop takes goes through
/// [`Self::advance`], which first emits whatever backing operations fall
/// before that point. The two schedules interleave exactly as they would in
/// real time.
struct BackingLane {
    layouts: Vec<LineLayout>,
    unsung: Vec<u8>,
    ops: Vec<(f64, BackingOp)>,
    next: usize,
}

impl BackingLane {
    fn new(track: &[TimedLine], palette: &Palette) -> Self {
        let windows = backing_display_windows(track);
        let mut layouts = Vec::with_capacity(track.len());
        let mut unsung = Vec::with_capacity(track.len());
        let mut ops: Vec<(f64, BackingOp)> = Vec::new();
        for (k, (line, &(show, hide))) in track.iter().zip(windows.iter()).enumerate() {
            let layout = layout_line_scaled(&line.text, 1);
            let (unsung_idx, highlight_idx) = palette.singer_colors(line.singer);
            if hide > show {
                ops.push((show, BackingOp::Show(k)));
                let chars: Vec<char> = layout.text.chars().collect();
                // Per timing unit (syllables of `beau|ti|ful` separately).
                let spans = crate::lyrics::word_char_spans(&line.text);
                for (tw, &(offset, len)) in word_timings(line).iter().zip(spans.iter()) {
                    // A word that would only light up after this line has
                    // already lost the lane (cut short by the next backing
                    // line) must not draw over that next line's text.
                    if tw.highlight_at >= hide {
                        continue;
                    }
                    for c in 0..len {
                        let char_idx = offset + c;
                        if char_idx >= chars.len() {
                            break; // clipped off the end of a too-long line
                        }
                        ops.push((
                            tw.highlight_at.max(show),
                            BackingOp::Highlight {
                                k,
                                char_idx,
                                ch: chars.get(char_idx).copied().unwrap_or(' '),
                                color: highlight_idx,
                            },
                        ));
                    }
                }
                ops.push((hide, BackingOp::Clear));
            }
            layouts.push(layout);
            unsung.push(unsung_idx);
        }
        // Stable sort - ops pushed in order for line k all precede line
        // k+1's at the same instant, so a line's Clear at exactly the next
        // line's Show time still runs first.
        ops.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        Self {
            layouts,
            unsung,
            ops,
            next: 0,
        }
    }

    /// Advances `w` to `t`, emitting every pending backing-lane operation
    /// scheduled at or before `t` along the way.
    fn advance(&mut self, w: &mut CdgWriter, t: f64) {
        self.emit_until(w, t);
        w.advance_to(t);
    }

    /// Emits every pending operation scheduled at or before `t`, without
    /// moving the writer past the last one emitted.
    fn emit_until(&mut self, w: &mut CdgWriter, t: f64) {
        while let Some((at, op)) = self.ops.get(self.next) {
            if *at > t {
                break;
            }
            w.advance_to(*at);
            match *op {
                BackingOp::Show(k) => {
                    clear_row(w, BACKING_ROW);
                    draw_row_scaled(w, BACKING_ROW, &self.layouts[k], BG, self.unsung[k]);
                }
                BackingOp::Highlight {
                    k,
                    char_idx,
                    ch,
                    color,
                } => {
                    draw_char_at(w, BACKING_ROW, &self.layouts[k], char_idx, ch, BG, color);
                }
                BackingOp::Clear => clear_row(w, BACKING_ROW),
            }
            self.next += 1;
        }
    }
}

/// Lay out `text` at `preferred_scale`, automatically falling back to 1x
/// if the text is too long to fit the 48-column canvas at that size (this
/// is the "shrink if it starts clipping" behavior).
fn layout_line_scaled(text: &str, preferred_scale: u8) -> LineLayout {
    // Display text: single spaces, syllable marks (`beau|ti|ful`) removed.
    let joined = crate::lyrics::normalize_text(text);
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
/// since [`draw_legend_row`] skips drawing whitespace). Each singer's label
/// uses their *unsung* (upcoming) color index, not their highlight one -
/// same reasoning as `video.rs`'s own `legend_text_and_colors`: the legend
/// appears before anything has been sung, so the highlight color (only
/// ever seen once a line is already playing) isn't what the viewer is
/// actually about to see.
fn legend_text_and_colors(
    palette: &Palette,
    singers: &[(Singer, Option<String>)],
) -> (String, Vec<u8>) {
    let mut text = String::new();
    let mut colors = Vec::new();
    for (i, (singer, custom_name)) in singers.iter().enumerate() {
        if i > 0 {
            text.push_str("  ");
            colors.extend([BG, BG]);
        }
        let (unsung, _) = palette.singer_colors(*singer);
        let label = effective_singer_label(*singer, custom_name);
        text.push_str(label);
        colors.extend(std::iter::repeat_n(unsung, label.chars().count()));
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

/// One pending tile write: (row, col, color0, color1, pixels).
type TileWrite = (u8, u8, u8, u8, TilePixels);

/// The tile writes needed to make the `rows`-tall band at `band_row` show
/// exactly `layout` in `color1` (or nothing, for `None`) with blank
/// background everywhere else - only tiles that actually differ from
/// what's on screen now (see [`CdgWriter::tile_differs`]). Writing a new
/// line over the old one this way, instead of clearing the whole band and
/// then drawing, costs only the tiles that change - and knowing the count
/// up front lets the caller start early enough to finish on time.
fn band_writes(
    w: &CdgWriter,
    band_row: u8,
    rows: u8,
    layout: Option<&LineLayout>,
    color1: u8,
) -> Vec<TileWrite> {
    let cols = SAFE_COLS as usize;
    let mut grid: Vec<(u8, TilePixels)> = vec![(BG, BLANK_TILE); cols * rows as usize];
    if let Some(layout) = layout {
        let scale = layout.scale as usize;
        let mut col = layout.start_col as usize;
        for ch in layout.text.chars() {
            if col + scale > cols {
                break;
            }
            let tiles = font::glyph_tile_scaled(ch, layout.scale);
            for (tr, tile_row) in tiles.iter().enumerate().take(rows as usize) {
                for (tc, tile) in tile_row.iter().enumerate() {
                    grid[tr * cols + col + tc] = (color1, *tile);
                }
            }
            col += scale;
        }
    }
    grid.into_iter()
        .enumerate()
        .map(|(i, (c1, px))| (band_row + (i / cols) as u8, (i % cols) as u8, BG, c1, px))
        .filter(|&(r, c, c0, c1, px)| w.tile_differs(r, c, c0, c1, &px))
        .collect()
}

fn apply_writes(w: &mut CdgWriter, writes: &[TileWrite]) {
    for (r, c, c0, c1, px) in writes {
        w.tile_block(*r, *c, *c0, *c1, px);
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

/// How long after the last line (main or backing) finishes the outro card
/// appears - a beat to let the last line register first.
pub const OUTRO_GAP_SECS: f64 = 1.5;
/// The outro card is only shown if it would be up for at least this long -
/// a song that ends right after its last line has no room for one.
pub const MIN_OUTRO_SECS: f64 = 2.0;

/// A resolved outro card, ready to draw: shown over `[start, end)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Outro {
    pub start: f64,
    pub end: f64,
    /// The card's text lines (see `OutroSettings::text_lines`).
    pub lines: Vec<String>,
    /// Also show the title and "by artist" (when the project has them).
    pub show_title: bool,
}

/// When the outro card shows: from [`OUTRO_GAP_SECS`] after the last main
/// or backing line finishes being sung, to the end of the song - or
/// `None` if there are no lines, or less than [`MIN_OUTRO_SECS`] left.
pub fn outro_window(
    timed_lines: &[TimedLine],
    backing_track: &[TimedLine],
    total_duration: f64,
) -> Option<(f64, f64)> {
    let last = timed_lines
        .iter()
        .chain(backing_track)
        .map(|l| l.sing_end)
        .fold(None, |acc: Option<f64>, e| {
            Some(acc.map_or(e, |a| a.max(e)))
        })?;
    let start = last + OUTRO_GAP_SECS;
    (total_duration - start >= MIN_OUTRO_SECS).then_some((start, total_duration))
}

/// The [`Outro`] to draw for these settings, or `None` if it's turned off,
/// has nothing to show, or doesn't fit (see [`outro_window`]).
pub fn resolve_outro(
    settings: &crate::lyrics::OutroSettings,
    timed_lines: &[TimedLine],
    backing_track: &[TimedLine],
    total_duration: f64,
    has_title_or_artist: bool,
) -> Option<Outro> {
    if !settings.enabled {
        return None;
    }
    let lines = settings.text_lines();
    let show_title = settings.show_title && has_title_or_artist;
    if lines.is_empty() && !show_title {
        return None;
    }
    let (start, end) = outro_window(timed_lines, backing_track, total_duration)?;
    Some(Outro {
        start,
        end,
        lines,
        show_title,
    })
}

/// The `.cdg` outro card: every row cleared, then the text lines (the
/// first at 2x if it fits), then the title (2x if it fits) and "by artist",
/// stacked and roughly centered in the safe area. Written as a diff against
/// the screen (see [`band_writes`]), so it only costs the tiles that change.
fn draw_cdg_outro(w: &mut CdgWriter, outro: &Outro, title: Option<&str>, artist: Option<&str>) {
    // (text, preferred scale, color index)
    let mut items: Vec<(String, u8, u8)> = Vec::new();
    for (k, line) in outro.lines.iter().enumerate() {
        items.push((
            line.clone(),
            if k == 0 { PREFERRED_SCALE } else { 1 },
            TITLE,
        ));
    }
    if outro.show_title {
        if let Some(t) = title {
            items.push((t.to_string(), PREFERRED_SCALE, TITLE));
        }
        if let Some(a) = artist {
            items.push((format!("by {a}"), 1, ARTIST));
        }
    }
    let layouts: Vec<(LineLayout, u8)> = items
        .iter()
        .map(|(text, scale, color)| (layout_line_scaled(text, *scale), *color))
        .collect();
    // One blank row between items; center the whole stack vertically.
    let height: u8 = layouts
        .iter()
        .map(|(l, _)| l.scale + 1)
        .sum::<u8>()
        .saturating_sub(1);
    let safe_rows = crate::cdg::SAFE_ROWS;
    let mut row = safe_rows.saturating_sub(height) / 2;
    let mut covered = [false; crate::cdg::SAFE_ROWS as usize];
    for (layout, color) in &layouts {
        if row + layout.scale > safe_rows {
            break;
        }
        let writes = band_writes(w, row, layout.scale, Some(layout), *color);
        apply_writes(w, &writes);
        for r in row..row + layout.scale {
            covered[r as usize] = true;
        }
        row += layout.scale + 1;
    }
    for r in 0..safe_rows {
        if !covered[r as usize] {
            let writes = band_writes(w, r, 1, None, BG);
            apply_writes(w, &writes);
        }
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
fn schedule_countdown_dots(
    w: &mut CdgWriter,
    lane: &mut BackingLane,
    cd_start: f64,
    cd_end: f64,
    lit_color: u8,
) {
    lane.advance(w, cd_start);
    let cols = countdown_columns();
    for &col in &cols {
        draw_countdown_dot(w, col, PREVIEW);
    }
    for (k, &col) in cols.iter().enumerate() {
        let t = cd_start + (k as f64) * (cd_end - cd_start) / 4.0;
        lane.advance(w, t);
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
        &[],
        total_duration,
        palette,
        title,
        artist,
        &TimingSettings::default(),
        None,
    )
}

/// Render a complete karaoke CDG stream, padded to `total_duration` seconds.
/// `backing_track` is the resolved backing track (see
/// [`crate::lyrics::resolve_backing_track`]), drawn on its own row
/// concurrently with `timed_lines` (the main track).
#[allow(clippy::too_many_arguments)]
pub fn render_cdg_with_settings(
    timed_lines: &[TimedLine],
    backing_track: &[TimedLine],
    total_duration: f64,
    palette: &Palette,
    title: Option<&str>,
    artist: Option<&str>,
    timing_settings: &TimingSettings,
    outro: Option<&Outro>,
) -> Vec<u8> {
    let mut w = CdgWriter::new();
    let mut lane = BackingLane::new(backing_track, palette);

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
    let legend_singers = singer_legend(timed_lines, backing_track);
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
    let has_card = title.is_some() || artist.is_some() || !legend_singers.is_empty();
    // When the first line starts right as the card ends (a first line
    // within the card's 5 seconds), clearing the card is folded into that
    // line's own early-start batch below instead - clearing it *at* the
    // line's start would push the line's own drawing late.
    let card_clear_with_first_line = has_card
        && timed_lines
            .first()
            .is_some_and(|first| card_end >= first.start);
    if has_card && !card_clear_with_first_line {
        lane.advance(&mut w, card_end);
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
            schedule_countdown_dots(&mut w, &mut lane, cd_start, cd_end, highlight_idx);
            lane.advance(&mut w, cd_end);
            clear_row(&mut w, COUNTDOWN_ROW);
        }
    }

    for (i, line) in timed_lines.iter().enumerate() {
        let (unsung_idx, highlight_idx) = palette.singer_colors(line.singer);
        let layout = layout_line_scaled(&line.text, PREFERRED_SCALE);

        // Everything that must be on screen when this line starts: the line
        // itself (written over whatever the band showed - see
        // `band_writes`), the countdown dots gone, and (first line only)
        // the title card gone. A CDG stream draws one tile per 1/300s, so
        // start that many packets *before* `line.start` - otherwise the
        // line (and its first word's highlight, queued behind it) would
        // land late by however long the drawing takes.
        let mut writes = band_writes(&w, CURRENT_ROW, PREFERRED_SCALE, Some(&layout), unsung_idx);
        writes.extend(band_writes(&w, COUNTDOWN_ROW, 1, None, BG));
        if i == 0 && card_clear_with_first_line {
            writes.extend(band_writes(&w, TITLE_ROW, PREFERRED_SCALE, None, BG));
            writes.extend(band_writes(&w, ARTIST_ROW, 1, None, BG));
            writes.extend(band_writes(&w, LEGEND_ROW, 1, None, BG));
        }
        let lead = writes.len() as f64 / crate::cdg::PACKETS_PER_SEC;
        lane.advance(&mut w, (line.start - lead).max(0.0));
        apply_writes(&mut w, &writes);

        // The dim next-line preview isn't needed at the line's exact start,
        // so it's drawn just after the first word lights (below) rather
        // than delaying that first highlight.
        let next_layout = timed_lines
            .get(i + 1)
            .map(|next| layout_line_scaled(&next.text, 1));
        let mut preview_drawn = false;

        // Per timing unit (syllables of `beau|ti|ful` separately).
        let spans = crate::lyrics::word_char_spans(&line.text);
        let words = word_timings(line);
        let chars: Vec<char> = layout.text.chars().collect();
        let mut events: Vec<HighlightEvent> = Vec::new();
        for (tw, &(offset, len)) in words.iter().zip(spans.iter()) {
            for k in 0..len {
                let char_idx = offset + k;
                if char_idx >= chars.len() {
                    break; // clipped off the end of a too-long line
                }
                events.push(HighlightEvent {
                    at: tw.highlight_at,
                    char_idx,
                    ch: chars.get(char_idx).copied().unwrap_or(' '),
                    highlight_idx,
                });
            }
        }
        // Manual word overrides can put words slightly out of order -
        // `advance_to` only moves forward, so emit in time order.
        events.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
        let first_at = events.first().map(|e| e.at);
        for ev in &events {
            if !preview_drawn && Some(ev.at) != first_at {
                let writes = band_writes(&w, PREVIEW_ROW, 1, next_layout.as_ref(), PREVIEW);
                apply_writes(&mut w, &writes);
                preview_drawn = true;
            }
            lane.advance(&mut w, ev.at);
            draw_char_at(
                &mut w,
                CURRENT_ROW,
                &layout,
                ev.char_idx,
                ev.ch,
                BG,
                ev.highlight_idx,
            );
        }

        if !preview_drawn {
            let writes = band_writes(&w, PREVIEW_ROW, 1, next_layout.as_ref(), PREVIEW);
            apply_writes(&mut w, &writes);
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
            lane.advance(&mut w, line.sing_end);
            clear_row(&mut w, PREVIEW_ROW);

            // Let the just-finished line linger fully highlighted for a
            // bit, then clear it too (or immediately, if the break is short
            // enough that the countdown starts before the linger would
            // finish) - so a long break just shows a blank screen until the
            // countdown appears, instead of sitting there the whole time.
            let blank_at = (line.sing_end + SUNG_LINGER_SECS).min(cd_start);
            lane.advance(&mut w, blank_at);
            clear_band(&mut w, CURRENT_ROW, PREFERRED_SCALE);

            // Only actually draw the countdown dots if there's a real next
            // line to count into - for the last line of the song, this same
            // "long gap" is just trailing silence after the song ends, with
            // nothing to cue the singer to get ready for. The screen still
            // blanks during that silence (above); it just doesn't show dots
            // counting down to a line that doesn't exist.
            if let Some(next) = timed_lines.get(i + 1) {
                let (_, next_highlight_idx) = palette.singer_colors(next.singer);
                schedule_countdown_dots(&mut w, &mut lane, cd_start, cd_end, next_highlight_idx);
            }
        }
    }

    // The outro card replaces everything on screen once the last line
    // (main and backing) is done - see `outro_window`.
    if let Some(outro) = outro {
        lane.advance(&mut w, outro.start);
        draw_cdg_outro(&mut w, outro, title, artist);
    }

    // Whatever's left of the backing lane after the main track's last
    // step - but nothing past the end of the song itself, which would only
    // pad the file with an extra clear after the audio has stopped.
    let song_end = total_duration.max(w.current_time_secs());
    lane.emit_until(&mut w, song_end);
    w.pad_until(total_duration.max(w.current_time_secs()));
    w.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outro_window_follows_the_last_line_main_or_backing() {
        let mut a = TimedLine::new("a".to_string(), 1.0, 5.0, Singer::Default);
        a.sing_end = 4.0;
        let mut back = TimedLine::new("b".to_string(), 3.0, 8.0, Singer::Backing);
        back.sing_end = 7.0;
        assert_eq!(outro_window(&[], &[], 60.0), None);
        assert_eq!(outro_window(&[a.clone()], &[], 60.0), Some((5.5, 60.0)));
        // A backing line finishing later pushes it back.
        assert_eq!(outro_window(&[a.clone()], &[back], 60.0), Some((8.5, 60.0)));
        // Under MIN_OUTRO_SECS left: no card.
        assert_eq!(outro_window(&[a.clone()], &[], 7.0), None);
        assert_eq!(outro_window(&[a], &[], 7.5), Some((5.5, 7.5)));
    }

    #[test]
    fn resolve_outro_respects_settings() {
        let mut a = TimedLine::new("a".to_string(), 1.0, 5.0, Singer::Default);
        a.sing_end = 4.0;
        let mut s = crate::lyrics::OutroSettings::default();
        assert_eq!(
            resolve_outro(&s, &[a.clone()], &[], 60.0, true),
            None,
            "off by default"
        );
        s.enabled = true;
        let o = resolve_outro(&s, &[a.clone()], &[], 60.0, true).unwrap();
        assert_eq!(o.lines, vec!["Thanks for singing!".to_string()]);
        assert!(o.show_title);
        // No title/artist to show and no text: nothing to draw.
        s.text = "  \n ".to_string();
        assert_eq!(resolve_outro(&s, &[a.clone()], &[], 60.0, false), None);
        // Text is trimmed and capped.
        s.text = " one \n\ntwo\nthree\nfour".to_string();
        let o = resolve_outro(&s, &[a], &[], 60.0, false).unwrap();
        assert_eq!(o.lines, vec!["one", "two", "three"]);
        assert!(!o.show_title);
    }
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
            crate::lyrics::singer_legend(&timed, &[]).len(),
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
        assert!(crate::lyrics::singer_legend(&timed, &[]).is_empty());
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
        // default_unsung, default_highlight, backing_unsung,
        // backing_highlight - all 8 used, nothing left free.
    }

    #[test]
    fn abyssal_preset_stays_within_the_clut_budgets() {
        let p = Palette::abyssal();
        assert_eq!(p.low_clut().len(), 8);
        assert_eq!(p.high_clut().len(), 8);
    }

    #[test]
    fn abyssal_preset_matches_the_requested_colors_as_closely_as_4_bit_allows() {
        // background #000000, unsung/default ~#BB0000 (originally #8B0000,
        // brightened for ~3:1 readability on black - see the preset's own
        // comments), highlight #FFB000 - confirms the nearest-4-bit-step
        // math is actually what's encoded, not just what the comment claims.
        let p = Palette::abyssal();
        assert_eq!((p.background.r, p.background.g, p.background.b), (0, 0, 0));
        assert_eq!(
            (p.default_unsung.r, p.default_unsung.g, p.default_unsung.b),
            (11, 0, 0)
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
            ("backing", p.backing_unsung, p.backing_highlight),
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

    fn render_with_backing(lines: &[LyricLine], total: f64) -> Vec<u8> {
        let settings = TimingSettings::default();
        let timed = crate::lyrics::resolve_timing_with_settings(lines, Some(total), &settings);
        let backing = crate::lyrics::resolve_backing_track(lines, &settings);
        render_cdg_with_settings(
            &timed,
            &backing,
            total,
            &Palette::default(),
            None,
            None,
            &settings,
            None,
        )
    }

    fn backing_line(text: &str, start: f64, end: f64) -> LyricLine {
        let mut l = LyricLine::new(text);
        l.is_backing = true;
        l.start = Some(start);
        l.sing_end_override = Some(end);
        l.singer = Singer::Backing;
        l
    }

    #[test]
    fn a_backing_line_draws_on_its_own_row() {
        let mut lines = vec![LyricLine::new("hello world"), backing_line("hey", 1.0, 3.0)];
        lines[0].start = Some(0.0);
        let bytes = render_with_backing(&lines, 10.0);
        assert_eq!(bytes.len() % 24, 0);
        assert!(
            non_blank_draws_at_row(&bytes, BACKING_ROW) > 0,
            "expected the backing line to actually draw something on its row"
        );
    }

    #[test]
    fn a_song_without_backing_lines_draws_nothing_on_the_backing_row() {
        let mut lines = vec![LyricLine::new("hello world")];
        lines[0].start = Some(0.0);
        let bytes = render_with_backing(&lines, 10.0);
        assert_eq!(non_blank_draws_at_row(&bytes, BACKING_ROW), 0);
    }

    #[test]
    fn a_backing_line_spanning_several_main_lines_renders_validly() {
        // Starts during main line 1, runs all the way through main line 3 -
        // impossible with the old host-bounded echo.
        let mut lines = vec![
            LyricLine::new("one"),
            backing_line("a long scream over everything", 0.5, 11.0),
            LyricLine::new("two"),
            LyricLine::new("three"),
        ];
        lines[0].start = Some(0.0);
        lines[2].start = Some(4.0);
        lines[3].start = Some(8.0);
        let bytes = render_with_backing(&lines, 15.0);
        assert_eq!(bytes.len() % 24, 0);
        // Every main line still draws, and so does the backing line.
        assert!(non_blank_draws_at_row(&bytes, BACKING_ROW) > 0);
        assert!(non_blank_draws_at_row(&bytes, CURRENT_ROW) > 0);
    }

    #[test]
    fn backing_lines_render_even_with_no_main_track_playing() {
        let lines = vec![backing_line("only me", 2.0, 4.0)];
        let bytes = render_with_backing(&lines, 6.0);
        assert!(non_blank_draws_at_row(&bytes, BACKING_ROW) > 0);
    }

    #[test]
    fn a_backing_line_uses_its_own_singers_colors() {
        let build = |backing_singer: Singer| {
            let mut lines = vec![LyricLine::new("hello world"), backing_line("hey", 1.0, 3.0)];
            lines[0].start = Some(0.0);
            lines[0].singer = Singer::Male;
            lines[1].singer = backing_singer;
            render_with_backing(&lines, 10.0)
        };
        assert_ne!(build(Singer::Duet), build(Singer::Backing));
    }

    #[test]
    fn the_backing_voice_has_its_own_clut_slots() {
        let p = Palette::default();
        assert_eq!(
            p.singer_colors(Singer::Backing),
            (BACKING_UNSUNG, BACKING_HIGHLIGHT)
        );
        assert_eq!(p.high_clut()[6].r, p.backing_unsung.r);
        assert_eq!(p.high_clut()[7].b, p.backing_highlight.b);
    }
}
