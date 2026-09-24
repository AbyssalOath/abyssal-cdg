//! Renders an actual video file (MP4, H.264 + AAC) of the karaoke display,
//! at a real modern resolution with anti-aliased text - the "looks like a
//! KaraFun/karadeo.com video" export, as opposed to the legacy `.cdg`
//! format (which is hard-capped at 300x216 pixels; see `export.rs`).
//!
//! Unlike the CDG path (which only has room for a "current line" + a dim
//! one-line preview), video has plenty of room, so this shows a whole
//! group of consecutive lines at once - like a real karaoke video's verse
//! block - with already-sung lines fully colored, the active line
//! smoothly wiping left-to-right through its own words, and upcoming
//! lines dim. Lines are grouped into blocks of up to 5 at the same
//! "musical break" points used for the countdown indicator, so a block
//! always corresponds to one uninterrupted run of singing.
//!
//! Approach: render each frame as a raw RGB24 pixel buffer using `ab_glyph`
//! for text (embedded DejaVu Sans / DejaVu Sans Bold - see `assets/`,
//! bundled under the permissive Bitstream Vera license), then pipe frames
//! into an `ffmpeg` subprocess via stdin, muxed with the loaded audio file.
//! `ffmpeg` itself is bundled with the app rather than required on the
//! system PATH - see `ffmpeg_path.rs` for how it's located (and why the
//! H.264 encoder it uses, openh264, is resolved separately).

#[cfg(test)]
use crate::lyrics::MAX_BLOCK_LINES;
use crate::lyrics::{
    backing_vocal_wipe_fraction, blank_sung_lines, countdown_window, countdown_window_between,
    current_line_wipe_fraction, group_into_blocks, hide_upcoming_lines, normalize_text,
    singer_legend, Singer, TimedLine, TimingSettings,
};
use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use anyhow::{anyhow, bail, Context, Result};
use std::io::{Read, Write};
use std::path::Path;
use std::process::Stdio;

static DEJAVU_REGULAR: &[u8] = include_bytes!("../assets/fonts/dejavu/DejaVuSans.ttf");
static DEJAVU_BOLD: &[u8] = include_bytes!("../assets/fonts/dejavu/DejaVuSans-Bold.ttf");

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Resolution {
    Hd1080,
    Uhd4k,
}

impl Resolution {
    pub fn dimensions(self) -> (u32, u32) {
        match self {
            Resolution::Hd1080 => (1920, 1080),
            Resolution::Uhd4k => (3840, 2160),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Resolution::Hd1080 => "1080p",
            Resolution::Uhd4k => "4K",
        }
    }
}

/// A user-picked image or video shown behind the lyrics in the video export
/// (and the live preview) instead of a flat `palette.background` fill -
/// album art, a music video, etc. Never applies to the legacy `.cdg`
/// export, which is hard-capped at a 300x216, 16-color tile display with no
/// room for arbitrary imagery.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Background {
    /// A still image, scaled/cropped to fill the frame and held for the
    /// whole song.
    Image(std::path::PathBuf),
    /// A video, scaled/cropped to fill the frame and decoded frame-by-frame
    /// in step with the export; looped if it's shorter than the song.
    Video(std::path::PathBuf),
}

impl Background {
    pub fn path(&self) -> &Path {
        match self {
            Self::Image(p) | Self::Video(p) => p,
        }
    }

    /// Guesses which kind of background a file is from its extension -
    /// used by the "choose a background" file dialog, which accepts both
    /// kinds through one filter. `None` for an extension that's neither a
    /// still image nor a video format the export pipeline (`image` crate /
    /// `ffmpeg`) can actually read.
    pub fn from_path(path: std::path::PathBuf) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp" | "tif" | "tiff" => {
                Some(Self::Image(path))
            }
            "mp4" | "mov" | "mkv" | "avi" | "webm" | "m4v" => Some(Self::Video(path)),
            _ => None,
        }
    }
}

/// How a background image/video that doesn't already match the frame's
/// aspect ratio gets fit into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum BackgroundFit {
    /// Scales up to completely fill the frame, cropping off whatever
    /// overflows (centered) - never distorts the image/video, but can crop
    /// off part of it. The default, and how most video editors handle a
    /// background clip that doesn't match the canvas.
    #[default]
    Cover,
    /// Scales down to fit entirely within the frame, padding the leftover
    /// space (letterboxed/pillarboxed, centered) with the palette's
    /// background color - nothing is ever cropped, at the cost of visible
    /// bars when the aspect ratios don't match.
    Contain,
}

impl BackgroundFit {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cover => "Cover (fills the frame, may crop edges)",
            Self::Contain => "Contain (shows all of it, may add bars)",
        }
    }
}

/// Full 8-bit-per-channel color - video isn't limited to CDG's 4-bit
/// palette, so colors you pick in the UI are used at full fidelity here.
#[derive(Clone, Copy, Debug)]
pub struct Rgb8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb8 {
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    fn lerp(self, other: Rgb8, t: f32) -> Rgb8 {
        let t = t.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Rgb8::new(
            mix(self.r, other.r),
            mix(self.g, other.g),
            mix(self.b, other.b),
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct VideoPalette {
    pub background: Rgb8,
    /// Color for a line whose singer hasn't been manually set - defaults to
    /// the same look as `male_unsung`/`male_highlight`, but independently
    /// adjustable (e.g. to match a background image/video's palette).
    pub default_unsung: Rgb8,
    pub default_highlight: Rgb8,
    pub male_unsung: Rgb8,
    pub male_highlight: Rgb8,
    pub female_unsung: Rgb8,
    pub female_highlight: Rgb8,
    pub duet_unsung: Rgb8,
    pub duet_highlight: Rgb8,
    pub preview: Rgb8,
    pub title: Rgb8,
    pub artist: Rgb8,
    pub screaming_unsung: Rgb8,
    pub screaming_highlight: Rgb8,
}

impl VideoPalette {
    fn singer_colors(&self, s: Singer) -> (Rgb8, Rgb8) {
        match s {
            Singer::Default => (self.default_unsung, self.default_highlight),
            Singer::Male => (self.male_unsung, self.male_highlight),
            Singer::Female => (self.female_unsung, self.female_highlight),
            Singer::Duet => (self.duet_unsung, self.duet_highlight),
            Singer::Screaming => (self.screaming_unsung, self.screaming_highlight),
        }
    }
}

/// A simple RGB24 frame buffer, row-major, top-to-bottom.
struct Canvas {
    w: usize,
    h: usize,
    buf: Vec<u8>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            buf: vec![0u8; w * h * 3],
        }
    }

    fn fill(&mut self, color: Rgb8) {
        for px in self.buf.chunks_mut(3) {
            px[0] = color.r;
            px[1] = color.g;
            px[2] = color.b;
        }
    }

    /// Overwrites the whole canvas with an already-sized RGB24 buffer (row-
    /// major, top-to-bottom, exactly `w * h * 3` bytes) - used to drop a
    /// decoded background image/video frame in before drawing lyrics on top
    /// of it. Silently no-ops on a size mismatch (e.g. a background video's
    /// decoder came up short on its very last frame) rather than panicking
    /// mid-export; the canvas just keeps whatever was already in it.
    fn set_from_rgb24(&mut self, data: &[u8]) {
        if data.len() == self.buf.len() {
            self.buf.copy_from_slice(data);
        }
    }

    /// Blends a flat color across the *entire* canvas at `alpha` - used to
    /// dim an image/video background enough that lyric text drawn on top of
    /// it (via [`Canvas::blend_pixel`]) stays legible instead of fighting
    /// with busy/bright footage.
    fn dim(&mut self, color: Rgb8, alpha: f32) {
        let a = alpha.clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        for px in self.buf.chunks_mut(3) {
            px[0] = (px[0] as f32 * (1.0 - a) + color.r as f32 * a).round() as u8;
            px[1] = (px[1] as f32 * (1.0 - a) + color.g as f32 * a).round() as u8;
            px[2] = (px[2] as f32 * (1.0 - a) + color.b as f32 * a).round() as u8;
        }
    }

    fn blend_pixel(&mut self, x: i32, y: i32, color: Rgb8, alpha: f32) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let idx = (y as usize * self.w + x as usize) * 3;
        let a = alpha.clamp(0.0, 1.0);
        self.buf[idx] = (self.buf[idx] as f32 * (1.0 - a) + color.r as f32 * a).round() as u8;
        self.buf[idx + 1] =
            (self.buf[idx + 1] as f32 * (1.0 - a) + color.g as f32 * a).round() as u8;
        self.buf[idx + 2] =
            (self.buf[idx + 2] as f32 * (1.0 - a) + color.b as f32 * a).round() as u8;
    }
}

/// Draw a filled circle. Centers are rounded (not truncated) so dots that
/// land on a fractional pixel don't drift slightly off-center.
fn draw_filled_circle(canvas: &mut Canvas, cx: f32, cy: f32, radius: f32, color: Rgb8) {
    let cx_i = cx.round() as i32;
    let cy_i = cy.round() as i32;
    let r = radius.ceil() as i32;
    for dy in -r..=r {
        for dx in -r..=r {
            if (dx * dx + dy * dy) as f32 <= radius * radius {
                canvas.blend_pixel(cx_i + dx, cy_i + dy, color, 1.0);
            }
        }
    }
}

fn measure_width(font: &FontArc, scale: PxScale, text: &str) -> f32 {
    let scaled = font.as_scaled(scale);
    text.chars()
        .map(|c| scaled.h_advance(font.glyph_id(c)))
        .sum()
}

/// Draws `text` centered at `center_x`, with one color per character
/// (`colors[i]` for the i-th char of `text`; if there are fewer colors
/// than characters, the last one is reused). Shrinks the font uniformly if
/// the line would be wider than `max_width` - the "adjust font size if it
/// starts clipping" behavior.
#[allow(clippy::too_many_arguments)]
fn draw_text_line_chars(
    canvas: &mut Canvas,
    font: &FontArc,
    scale_px: f32,
    center_x: f32,
    baseline_y: f32,
    text: &str,
    colors: &[Rgb8],
    max_width: f32,
) {
    if text.is_empty() {
        return;
    }
    let mut scale_px = scale_px;
    let mut scale = PxScale::from(scale_px);
    let total_w = measure_width(font, scale, text);
    if total_w > max_width && total_w > 0.0 {
        scale_px *= max_width / total_w;
        scale = PxScale::from(scale_px);
    }
    let final_w = measure_width(font, scale, text);
    let scaled = font.as_scaled(scale);

    let fallback = colors.last().copied().unwrap_or(Rgb8::new(255, 255, 255));
    let mut x = center_x - final_w / 2.0;
    for (i, ch) in text.chars().enumerate() {
        let gid = font.glyph_id(ch);
        let advance = scaled.h_advance(gid);
        if !ch.is_whitespace() {
            let color = colors.get(i).copied().unwrap_or(fallback);
            let glyph = gid.with_scale_and_position(scale, ab_glyph::point(x, baseline_y));
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|gx, gy, coverage| {
                    let px = bounds.min.x as i32 + gx as i32;
                    let py = bounds.min.y as i32 + gy as i32;
                    canvas.blend_pixel(px, py, color, coverage);
                });
            }
        }
        x += advance;
    }
}

/// Draws `text` with a continuous left-to-right color wipe: pixels left of
/// the wipe boundary (`wipe_fraction` of the way across the line's own
/// rendered width) are `highlight`, pixels right of it are `unsung`, with a
/// few pixels of soft blending right at the boundary. Unlike coloring whole
/// characters at a time, this moves smoothly through every letter - including
/// mid-letter - as `wipe_fraction` increases, which is what actually reads
/// as "the color moving through the word" rather than words/letters
/// snapping to a new color.
#[allow(clippy::too_many_arguments)]
fn draw_text_line_wipe(
    canvas: &mut Canvas,
    font: &FontArc,
    scale_px: f32,
    center_x: f32,
    baseline_y: f32,
    text: &str,
    unsung: Rgb8,
    highlight: Rgb8,
    wipe_fraction: f32,
    max_width: f32,
) {
    if text.is_empty() {
        return;
    }
    let mut scale_px = scale_px;
    let mut scale = PxScale::from(scale_px);
    let total_w = measure_width(font, scale, text);
    if total_w > max_width && total_w > 0.0 {
        scale_px *= max_width / total_w;
        scale = PxScale::from(scale_px);
    }
    let final_w = measure_width(font, scale, text);
    let scaled = font.as_scaled(scale);

    let left_x = center_x - final_w / 2.0;
    let wipe_x = left_x + final_w * wipe_fraction.clamp(0.0, 1.0);
    // A few pixels of soft edge so the boundary doesn't look aliased -
    // scales gently with font size so it looks proportionate at 4K too.
    let blend_half_width = (scale_px * 0.035).max(1.5);

    let mut x = left_x;
    for ch in text.chars() {
        let gid = font.glyph_id(ch);
        let advance = scaled.h_advance(gid);
        if !ch.is_whitespace() {
            let glyph = gid.with_scale_and_position(scale, ab_glyph::point(x, baseline_y));
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|gx, gy, coverage| {
                    let px = bounds.min.x as i32 + gx as i32;
                    let py = bounds.min.y as i32 + gy as i32;
                    let dx = px as f32 - wipe_x;
                    let blend_t = (dx / blend_half_width * 0.5 + 0.5).clamp(0.0, 1.0);
                    let color = highlight.lerp(unsung, blend_t);
                    canvas.blend_pixel(px, py, color, coverage);
                });
            }
        }
        x += advance;
    }
}

/// Builds the singer-legend's display text (e.g. "Male   Female") plus a
/// per-character color to pass to [`draw_text_line_chars`] - each singer's
/// label is colored with that singer's own highlight color, so the legend
/// on the intro screen ties the colors used during the song to the voice
/// they represent.
fn legend_text_and_colors(palette: &VideoPalette, singers: &[Singer]) -> (String, Vec<Rgb8>) {
    let mut text = String::new();
    let mut colors = Vec::new();
    for (i, singer) in singers.iter().enumerate() {
        if i > 0 {
            text.push_str("   ");
            colors.extend([palette.preview; 3]);
        }
        let (_, highlight) = palette.singer_colors(*singer);
        let label = singer.label();
        text.push_str(label);
        colors.extend(std::iter::repeat_n(highlight, label.chars().count()));
    }
    (text, colors)
}

/// Convenience wrapper for a whole line in a single uniform color.
#[allow(clippy::too_many_arguments)]
fn draw_text_line_uniform(
    canvas: &mut Canvas,
    font: &FontArc,
    scale_px: f32,
    center_x: f32,
    baseline_y: f32,
    text: &str,
    color: Rgb8,
    max_width: f32,
) {
    let colors = vec![color; text.chars().count().max(1)];
    draw_text_line_chars(
        canvas, font, scale_px, center_x, baseline_y, text, &colors, max_width,
    );
}

/// Number of countdown dots lit (0..=4) for a countdown window that runs
/// from `cd_start` to `cd_end`, at time `t`. Dot `k` (0-indexed) lights up
/// at the *start* of its quarter of the window and stays lit through to
/// `cd_end`, so all 4 are lit for the entire final quarter rather than dot
/// 4 only appearing for a single instant right as the next line begins.
fn countdown_lit_count(cd_start: f64, cd_end: f64, t: f64) -> usize {
    let frac = ((t - cd_start) / (cd_end - cd_start).max(0.001)).clamp(0.0, 1.0);
    ((frac * 4.0).floor() as i64 + 1).clamp(0, 4) as usize
}

fn draw_countdown_dots(
    canvas: &mut Canvas,
    w: f32,
    h: f32,
    lit: usize,
    lit_color: Rgb8,
    dim_color: Rgb8,
) {
    let cy = h * 0.85;
    let radius = h * 0.012;
    let spacing = h * 0.05;
    for i in 0..4 {
        let cx = w / 2.0 + (i as f32 - 1.5) * spacing;
        let color = if i < lit { lit_color } else { dim_color };
        draw_filled_circle(canvas, cx, cy, radius, color);
    }
}

/// Draws one frame's lyrics/title-card/countdown content onto `canvas`.
/// Does *not* touch the background - the caller fills `canvas` first,
/// either with `palette.background` (the plain solid-color look) or a
/// decoded image/video background frame (optionally dimmed), so this
/// function's `blend_pixel`-based text drawing composites correctly over
/// either one. See [`render_video`].
#[allow(clippy::too_many_arguments)]
fn render_frame(
    canvas: &mut Canvas,
    regular: &FontArc,
    bold: &FontArc,
    timed_lines: &[TimedLine],
    blocks: &[Vec<usize>],
    palette: &VideoPalette,
    title: Option<&str>,
    artist: Option<&str>,
    card_end: f64,
    t: f64,
    timing_settings: &TimingSettings,
) {
    let w = canvas.w as f32;
    let h = canvas.h as f32;
    let max_width = w * 0.92;

    let legend_singers = singer_legend(timed_lines);
    let has_title_card = title.is_some() || artist.is_some() || !legend_singers.is_empty();
    if has_title_card && t < card_end {
        if let Some(ti) = title {
            draw_text_line_uniform(
                canvas,
                bold,
                h * 0.10,
                w / 2.0,
                h * 0.42,
                ti,
                palette.title,
                max_width,
            );
        }
        if let Some(a) = artist {
            let by = format!("by {a}");
            draw_text_line_uniform(
                canvas,
                regular,
                h * 0.05,
                w / 2.0,
                h * 0.52,
                &by,
                palette.artist,
                max_width,
            );
        }
        if !legend_singers.is_empty() {
            let (text, colors) = legend_text_and_colors(palette, &legend_singers);
            draw_text_line_chars(
                canvas,
                regular,
                h * 0.04,
                w / 2.0,
                h * 0.62,
                &text,
                &colors,
                max_width,
            );
        }
        return;
    }

    // Between the title card fading and the first line actually starting:
    // if that gap is long, show the same "get ready" countdown used for
    // mid-song breaks, instead of just sitting on a blank screen.
    if let Some(first) = timed_lines.first() {
        if t < first.start {
            if let Some((cd_start, cd_end)) = countdown_window_between(
                card_end,
                first.start,
                first.countdown_mode,
                timing_settings,
            ) {
                if t >= cd_start {
                    let (_, highlight) = palette.singer_colors(first.singer);
                    let lit = countdown_lit_count(cd_start, cd_end, t);
                    draw_countdown_dots(canvas, w, h, lit, highlight, palette.preview);
                }
            }
            return;
        }
    }

    let Some(current_idx) = timed_lines.iter().position(|l| t >= l.start && t < l.end) else {
        return;
    };
    let block = blocks
        .iter()
        .find(|b| b.contains(&current_idx))
        .cloned()
        .unwrap_or_else(|| vec![current_idx]);
    let slot_in_block = block.iter().position(|&i| i == current_idx).unwrap_or(0);

    // Once the current line is done being sung and there's a real musical
    // break before the next one (long enough to warrant the countdown
    // indicator below), hide the not-yet-started lines in this block
    // instead of leaving them sitting on screen the whole time - the screen
    // should read as "done, waiting" (then the countdown dots, then the
    // next line), not show lyrics that are still a break away. During a
    // long enough break, the already-sung lines get cleared too (after
    // lingering for a bit) instead of sitting there for the whole break.
    let next_countdown_mode = timed_lines
        .get(current_idx + 1)
        .map(|n| n.countdown_mode)
        .unwrap_or_default();
    let hide_upcoming = hide_upcoming_lines(
        &timed_lines[current_idx],
        t,
        next_countdown_mode,
        timing_settings,
    );
    let blank_sung = blank_sung_lines(
        &timed_lines[current_idx],
        t,
        next_countdown_mode,
        timing_settings,
    );

    let line_height = h * 0.11;
    let font_size = h * 0.055;
    let total_height = block.len() as f32 * line_height;
    let start_y = h * 0.5 - total_height / 2.0 + line_height * 0.5;

    for (slot, &idx) in block.iter().enumerate() {
        let line = &timed_lines[idx];
        let text = normalize_text(&line.text);
        let y = start_y + slot as f32 * line_height;
        let (unsung, highlight) = palette.singer_colors(line.singer);

        match slot.cmp(&slot_in_block) {
            std::cmp::Ordering::Less => {
                // Already sung - shown fully in the highlight color.
                if !blank_sung {
                    draw_text_line_uniform(
                        canvas,
                        regular,
                        font_size,
                        w / 2.0,
                        y,
                        &text,
                        highlight,
                        max_width,
                    );
                }
            }
            std::cmp::Ordering::Equal => {
                if !blank_sung {
                    let wipe_fraction = current_line_wipe_fraction(line, t);
                    draw_text_line_wipe(
                        canvas,
                        bold,
                        font_size * 1.05,
                        w / 2.0,
                        y,
                        &text,
                        unsung,
                        highlight,
                        wipe_fraction,
                        max_width,
                    );

                    // A backing/echo vocal (if any) draws directly beneath
                    // the current line, smaller, in its own color, with its
                    // own independent wipe - only while its own (bounded
                    // within the host's) window is actually active, same as
                    // the `.cdg` export's backing row.
                    if let Some(bv) = &line.backing_vocal {
                        if t >= bv.start && t < bv.end {
                            let (bv_unsung, bv_highlight) = palette.singer_colors(bv.singer);
                            let bv_text = normalize_text(&bv.text);
                            let bv_wipe = backing_vocal_wipe_fraction(bv, t);
                            draw_text_line_wipe(
                                canvas,
                                regular,
                                font_size * 0.7,
                                w / 2.0,
                                y + line_height * 0.5,
                                &bv_text,
                                bv_unsung,
                                bv_highlight,
                                bv_wipe,
                                max_width,
                            );
                        }
                    }
                }
            }
            std::cmp::Ordering::Greater => {
                if !hide_upcoming {
                    draw_text_line_uniform(
                        canvas,
                        regular,
                        font_size,
                        w / 2.0,
                        y,
                        &text,
                        unsung,
                        max_width,
                    );
                }
            }
        }
    }

    // Only draw the countdown if there's a real next line to count into -
    // for the last line of the song, a long gap here is just trailing
    // silence after the song ends, not a break before another line.
    let has_next_line = current_idx + 1 < timed_lines.len();
    if let Some((cd_start, cd_end)) = countdown_window(
        &timed_lines[current_idx],
        next_countdown_mode,
        timing_settings,
    ) {
        if has_next_line && t >= cd_start {
            let next_singer = timed_lines
                .get(current_idx + 1)
                .map(|l| l.singer)
                .unwrap_or(timed_lines[current_idx].singer);
            let (_, highlight) = palette.singer_colors(next_singer);
            let lit = countdown_lit_count(cd_start, cd_end, t);
            draw_countdown_dots(canvas, w, h, lit, highlight, palette.preview);
        }
    }
}

/// Confirms `ffmpeg` is available, with a clear, actionable error message
/// if not (video export needs it; the `.cdg` path doesn't). See
/// `ffmpeg_path.rs` for how it's located.
pub fn check_ffmpeg_available() -> Result<()> {
    crate::ffmpeg_path::check_available()
}

/// An `ffmpeg` `-vf` filter chain that fits a `w`x`h` frame according to
/// `fit` - either "cover" (scale to fill, cropping the overflow, centered)
/// or "contain" (scale to fit within, padding the leftover space with
/// `pad_color`, centered). Never distorts the source's own aspect ratio
/// either way.
fn fit_filter(fit: BackgroundFit, w: u32, h: u32, pad_color: Rgb8) -> String {
    match fit {
        BackgroundFit::Cover => {
            format!("scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}")
        }
        BackgroundFit::Contain => {
            let hex = format!(
                "0x{:02x}{:02x}{:02x}",
                pad_color.r, pad_color.g, pad_color.b
            );
            format!(
                "scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color={hex}"
            )
        }
    }
}

/// Decodes and fits a still-image background to exactly `w * h * 3` RGB24
/// bytes, ready to hand to [`Canvas::set_from_rgb24`] once per export (a
/// static image is the same on every frame, so this only runs once). Also
/// used by the live preview to build the same background at preview
/// resolution.
pub(crate) fn load_image_background(
    path: &Path,
    w: u32,
    h: u32,
    fit: BackgroundFit,
    pad_color: Rgb8,
) -> Result<Vec<u8>> {
    let img = image::open(path)
        .with_context(|| format!("couldn't read background image {}", path.display()))?;
    let rgb = match fit {
        BackgroundFit::Cover => img
            .resize_to_fill(w, h, image::imageops::FilterType::Lanczos3)
            .to_rgb8(),
        BackgroundFit::Contain => {
            // `resize` (unlike `resize_to_fill`) scales down to fit
            // *within* the bounds, preserving aspect ratio, so the result
            // may come up short in one dimension - pasted centered onto a
            // pad_color-filled canvas of exactly w x h to make up the rest.
            let scaled = img
                .resize(w, h, image::imageops::FilterType::Lanczos3)
                .to_rgb8();
            let mut canvas = image::RgbImage::from_pixel(
                w,
                h,
                image::Rgb([pad_color.r, pad_color.g, pad_color.b]),
            );
            let x = ((w - scaled.width()) / 2) as i64;
            let y = ((h - scaled.height()) / 2) as i64;
            image::imageops::overlay(&mut canvas, &scaled, x, y);
            canvas
        }
    };
    Ok(rgb.into_raw())
}

/// Spawns a dedicated `ffmpeg` process that does nothing but decode a
/// background video to a raw RGB24 frame stream, already fit to `w`x`h`
/// (see [`BackgroundFit`]) and resampled to `fps` - so the main export loop
/// can just read `w * h * 3`-byte frames from its stdout in lockstep with
/// its own rendering, exactly like it already reads nothing at all for a
/// plain solid-color background. `-stream_loop -1` loops the source
/// indefinitely, so a background video shorter than the song just repeats
/// rather than running out partway through.
fn spawn_background_video_decoder(
    path: &Path,
    w: u32,
    h: u32,
    fps: u32,
    fit: BackgroundFit,
    pad_color: Rgb8,
) -> Result<std::process::Child> {
    crate::ffmpeg_path::command()?
        .args(["-stream_loop", "-1", "-i"])
        .arg(path)
        .args([
            "-vf",
            &format!("{},fps={fps}", fit_filter(fit, w, h, pad_color)),
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "-an",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to launch ffmpeg to decode {}", path.display()))
}

/// Grabs a single early frame from a background video, fit to `w`x`h` (see
/// [`BackgroundFit`]), for the live in-app preview to show as a stand-in
/// for the real footage - full frame-accurate video playback inside that
/// small preview isn't worth the extra decoder/timing machinery it'd take,
/// when the actual export already plays the real video back in sync with
/// the song.
///
/// Tries a fraction of a second into the file first, not literal frame 0,
/// escalating through a few later points before finally trying frame 0
/// itself: some real-world files (confirmed directly - a video downloaded/
/// remuxed via a tool like `yt-dlp`, its container metadata naming Google
/// as the producer) have a leading stretch that fails to decode at all -
/// e.g. a title card or fade-in spliced in without a full re-encode,
/// leaving that whole stretch with different/incompatible stream
/// parameters from the rest of the file - which makes `-frames:v 1` with
/// no (or too small a) seek silently produce zero output frames, even
/// though the rest of the file decodes completely normally. A single fixed
/// seek offset isn't enough if that bad stretch runs longer than it - so
/// this escalates through a few points rather than trying just one, and
/// only genuinely fails once every one of them comes up empty.
const THUMBNAIL_SEEK_CASCADE: [Option<&str>; 5] =
    [Some("0.5"), Some("3"), Some("8"), Some("15"), None];

pub fn extract_video_background_thumbnail(
    path: &Path,
    w: u32,
    h: u32,
    fit: BackgroundFit,
    pad_color: Rgb8,
) -> Result<Vec<u8>> {
    check_ffmpeg_available()?;
    let expected_len = w as usize * h as usize * 3;
    let vf = fit_filter(fit, w, h, pad_color);

    let grab = |seek: Option<&str>| -> Result<std::process::Output> {
        let mut cmd = crate::ffmpeg_path::command()?;
        cmd.arg("-y");
        if let Some(seek) = seek {
            // Placed before `-i` for fast, keyframe-based input seeking -
            // this is a best-effort preview thumbnail, not a frame-exact
            // export, so an approximate seek is the right trade-off.
            cmd.args(["-ss", seek]);
        }
        cmd.arg("-i").arg(path).args([
            "-frames:v",
            "1",
            "-vf",
            &vf,
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "-",
        ]);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .with_context(|| format!("failed to run ffmpeg on {}", path.display()))
    };

    let mut attempts = Vec::with_capacity(THUMBNAIL_SEEK_CASCADE.len());
    for seek in THUMBNAIL_SEEK_CASCADE {
        let output = grab(seek)?;
        if output.status.success() && output.stdout.len() >= expected_len {
            return Ok(output.stdout);
        }
        attempts.push((seek, output));
    }

    // Every attempt came up empty - report each one's stderr tail (not
    // just the very last line or two: the actual diagnostic - a decoder
    // error, "moov atom not found", etc. - almost always appears well
    // before ffmpeg's final "Conversion failed!" summary, so too short a
    // tail can cut off the one line that actually explains what went
    // wrong), labeled by which seek point it was.
    let sections: Vec<String> = attempts
        .iter()
        .map(|(seek, output)| {
            let label = seek.map_or("from the start (frame 0)".to_string(), |s| {
                format!("seeked to {s}s")
            });
            let tail: String = String::from_utf8_lossy(&output.stderr)
                .lines()
                .rev()
                .take(15)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            format!("--- {label} ---\n{tail}")
        })
        .collect();
    bail!(
        "couldn't read a preview frame from {}:\n{}",
        path.display(),
        sections.join("\n")
    );
}

/// Renders the full karaoke video to `output_path`, muxed with the audio
/// at `audio_path`. Calls `on_progress(0.0..=1.0)` periodically so the UI
/// can show a progress bar - this can take anywhere from several seconds to
/// a few minutes depending on song length and resolution.
///
/// `custom_font_bytes`, if `Some`, is used for *both* the regular and bold
/// text roles in place of the bundled DejaVu Sans/Sans Bold - `ab_glyph`
/// has no synthetic-bold support, and a system font's family (as
/// enumerated by `fonts.rs`) isn't guaranteed to include a genuine bold
/// sibling face, so reusing the one loaded face for both is the honest
/// simplification: the currently-singing line stays visually distinct via
/// its slightly larger size and the color wipe, just not a heavier weight.
/// `None` keeps today's exact behavior (bundled DejaVu, regular + true bold).
#[allow(clippy::too_many_arguments)]
pub fn render_video(
    timed_lines: &[TimedLine],
    total_duration: f64,
    palette: &VideoPalette,
    title: Option<&str>,
    artist: Option<&str>,
    resolution: Resolution,
    fps: u32,
    audio_path: &Path,
    background: Option<&Background>,
    background_fit: BackgroundFit,
    background_dim: f32,
    custom_font_bytes: Option<Vec<u8>>,
    output_path: &Path,
    timing_settings: &TimingSettings,
    mut on_progress: impl FnMut(f32),
) -> Result<()> {
    check_ffmpeg_available()?;

    let (regular, bold) = match custom_font_bytes {
        Some(bytes) => {
            let font = FontArc::try_from_vec(bytes)
                .map_err(|_| anyhow!("failed to parse the selected custom font"))?;
            (font.clone(), font)
        }
        None => (
            FontArc::try_from_slice(DEJAVU_REGULAR)
                .context("failed to parse embedded regular font")?,
            FontArc::try_from_slice(DEJAVU_BOLD).context("failed to parse embedded bold font")?,
        ),
    };

    let (w, h) = resolution.dimensions();
    let card_end = crate::export::title_card_end(timed_lines);
    let blocks = group_into_blocks(timed_lines);
    let total_frames = ((total_duration * fps as f64).ceil() as u64).max(1);

    // Prepare whatever's going to fill the background of every frame, before
    // spawning the (potentially slow to start) encoder process.
    enum BackgroundSource {
        None,
        Image(Vec<u8>),
        Video {
            child: std::process::Child,
            stdout: std::process::ChildStdout,
            frame: Vec<u8>,
            scratch: Vec<u8>,
            got_first_frame: bool,
        },
    }
    impl BackgroundSource {
        /// Kills the background decoder's `ffmpeg` process (spawned with
        /// `-stream_loop -1`, so it never exits on its own) and reaps it -
        /// otherwise it'd sit there decoding forever once the main export
        /// loop stops reading its stdout, whether that's because the
        /// export finished or because it was aborted early.
        fn cleanup(&mut self) {
            if let Self::Video { child, .. } = self {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
    let mut bg_source = match background {
        None => BackgroundSource::None,
        Some(Background::Image(path)) => BackgroundSource::Image(load_image_background(
            path,
            w,
            h,
            background_fit,
            palette.background,
        )?),
        Some(Background::Video(path)) => {
            let mut child = spawn_background_video_decoder(
                path,
                w,
                h,
                fps,
                background_fit,
                palette.background,
            )?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| anyhow!("failed to open background decoder's stdout"))?;
            let frame_len = w as usize * h as usize * 3;
            BackgroundSource::Video {
                child,
                stdout,
                frame: vec![0u8; frame_len],
                scratch: vec![0u8; frame_len],
                got_first_frame: false,
            }
        }
    };

    // openh264 (this app's bundled H.264 encoder - see ffmpeg_path.rs for
    // why it's openh264 rather than the GPL-licensed x264 most ffmpeg
    // builds default to) is a simpler, bitrate-driven encoder without
    // x264's CRF-style "target a perceptual quality" mode, and its
    // rate-distortion optimization is genuinely weaker than x264's at the
    // same bitrate - so this targets a generously high bitrate to
    // compensate, rather than trying to replicate a CRF setting that
    // openh264 has no real equivalent for. Karaoke video content (mostly
    // static text over a slow-moving or still background) compresses well
    // in practice, so this is comfortably more headroom than the content
    // actually needs.
    let video_bitrate = match (w, h) {
        _ if w * h > 1920 * 1080 => "32M", // 4K
        _ => "10M",                        // 1080p (or anything smaller)
    };
    let mut child = crate::ffmpeg_path::command()?
        .args([
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s",
            &format!("{w}x{h}"),
            "-r",
            &fps.to_string(),
            "-i",
            "-",
            "-i",
        ])
        .arg(audio_path)
        .args([
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            "libopenh264",
            "-b:v",
            video_bitrate,
            "-profile:v",
            "high",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-shortest",
        ])
        .arg(output_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to launch ffmpeg")?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("failed to open ffmpeg stdin"))?;
    let mut canvas = Canvas::new(w as usize, h as usize);

    for frame_idx in 0..total_frames {
        let t = frame_idx as f64 / fps as f64;

        let mut fatal_bg_error: Option<std::io::Error> = None;
        match &mut bg_source {
            BackgroundSource::None => canvas.fill(palette.background),
            BackgroundSource::Image(buf) => canvas.set_from_rgb24(buf),
            BackgroundSource::Video {
                stdout,
                frame: bg_frame,
                scratch,
                got_first_frame,
                ..
            } => {
                match stdout.read_exact(scratch) {
                    Ok(()) => {
                        std::mem::swap(bg_frame, scratch);
                        *got_first_frame = true;
                    }
                    // A later hiccup/EOF just freezes on the last good
                    // frame instead of failing the whole export - only a
                    // failure to decode even the first frame is fatal.
                    Err(e) if !*got_first_frame => fatal_bg_error = Some(e),
                    Err(_) => {}
                }
                canvas.set_from_rgb24(bg_frame);
            }
        }
        if let Some(e) = fatal_bg_error {
            bg_source.cleanup();
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!("couldn't decode the background video: {e}"));
        }
        if background.is_some() {
            canvas.dim(Rgb8::new(0, 0, 0), background_dim);
        }

        render_frame(
            &mut canvas,
            &regular,
            &bold,
            timed_lines,
            &blocks,
            palette,
            title,
            artist,
            card_end,
            t,
            timing_settings,
        );
        if stdin.write_all(&canvas.buf).is_err() {
            break;
        }
        if frame_idx % (fps as u64).max(1) == 0 {
            on_progress(frame_idx as f32 / total_frames as f32);
        }
    }
    bg_source.cleanup();
    drop(stdin);
    on_progress(1.0);

    let output = child
        .wait_with_output()
        .context("failed waiting for ffmpeg to finish")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .lines()
            .rev()
            .take(15)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        bail!("ffmpeg failed:\n{tail}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::{resolve_timing, word_timings, LyricLine, Singer};

    #[test]
    fn background_from_path_recognizes_image_extensions_case_insensitively() {
        assert_eq!(
            Background::from_path(std::path::PathBuf::from("cover.PNG")),
            Some(Background::Image(std::path::PathBuf::from("cover.PNG")))
        );
        assert_eq!(
            Background::from_path(std::path::PathBuf::from("art.jpeg")),
            Some(Background::Image(std::path::PathBuf::from("art.jpeg")))
        );
    }

    #[test]
    fn background_from_path_recognizes_video_extensions() {
        assert_eq!(
            Background::from_path(std::path::PathBuf::from("clip.MP4")),
            Some(Background::Video(std::path::PathBuf::from("clip.MP4")))
        );
        assert_eq!(
            Background::from_path(std::path::PathBuf::from("clip.webm")),
            Some(Background::Video(std::path::PathBuf::from("clip.webm")))
        );
    }

    #[test]
    fn background_from_path_rejects_unsupported_or_missing_extensions() {
        assert_eq!(
            Background::from_path(std::path::PathBuf::from("notes.txt")),
            None
        );
        assert_eq!(
            Background::from_path(std::path::PathBuf::from("noext")),
            None
        );
    }

    #[test]
    fn background_path_returns_the_inner_path_for_either_kind() {
        let img = Background::Image(std::path::PathBuf::from("a.png"));
        let vid = Background::Video(std::path::PathBuf::from("b.mp4"));
        assert_eq!(img.path(), std::path::Path::new("a.png"));
        assert_eq!(vid.path(), std::path::Path::new("b.mp4"));
    }

    #[test]
    fn fit_filter_cover_scales_then_crops_to_the_target_size() {
        assert_eq!(
            fit_filter(BackgroundFit::Cover, 1920, 1080, Rgb8::new(0, 0, 0)),
            "scale=1920:1080:force_original_aspect_ratio=increase,crop=1920:1080"
        );
    }

    #[test]
    fn fit_filter_contain_scales_then_pads_with_the_given_color() {
        assert_eq!(
            fit_filter(BackgroundFit::Contain, 1920, 1080, Rgb8::new(18, 52, 86)),
            "scale=1920:1080:force_original_aspect_ratio=decrease,pad=1920:1080:(ow-iw)/2:(oh-ih)/2:color=0x123456"
        );
    }

    #[test]
    fn canvas_set_from_rgb24_replaces_the_buffer_on_a_matching_size() {
        let mut canvas = Canvas::new(2, 1);
        canvas.fill(Rgb8::new(1, 2, 3));
        let new_pixels = [10, 20, 30, 40, 50, 60];
        canvas.set_from_rgb24(&new_pixels);
        assert_eq!(canvas.buf, new_pixels);
    }

    #[test]
    fn canvas_set_from_rgb24_ignores_a_size_mismatch() {
        let mut canvas = Canvas::new(2, 1);
        canvas.fill(Rgb8::new(1, 2, 3));
        let wrong_size = [10, 20, 30];
        canvas.set_from_rgb24(&wrong_size);
        assert_eq!(canvas.buf, [1, 2, 3, 1, 2, 3]);
    }

    #[test]
    fn canvas_dim_blends_toward_the_scrim_color() {
        let mut canvas = Canvas::new(1, 1);
        canvas.fill(Rgb8::new(200, 200, 200));
        canvas.dim(Rgb8::new(0, 0, 0), 0.5);
        assert_eq!(canvas.buf, [100, 100, 100]);
    }

    #[test]
    fn canvas_dim_at_zero_alpha_is_a_no_op() {
        let mut canvas = Canvas::new(1, 1);
        canvas.fill(Rgb8::new(200, 100, 50));
        canvas.dim(Rgb8::new(0, 0, 0), 0.0);
        assert_eq!(canvas.buf, [200, 100, 50]);
    }

    #[test]
    fn legend_text_and_colors_uses_each_singers_highlight_color() {
        let palette = VideoPalette {
            background: Rgb8::new(5, 5, 20),
            default_unsung: Rgb8::new(230, 230, 230),
            default_highlight: Rgb8::new(255, 220, 0),
            male_unsung: Rgb8::new(230, 230, 230),
            male_highlight: Rgb8::new(255, 220, 0),
            female_unsung: Rgb8::new(210, 210, 255),
            female_highlight: Rgb8::new(255, 90, 220),
            duet_unsung: Rgb8::new(200, 255, 200),
            duet_highlight: Rgb8::new(255, 150, 0),
            preview: Rgb8::new(110, 110, 160),
            title: Rgb8::new(255, 255, 255),
            artist: Rgb8::new(160, 160, 200),
            screaming_unsung: Rgb8::new(140, 30, 20),
            screaming_highlight: Rgb8::new(255, 60, 10),
        };
        let (text, colors) = legend_text_and_colors(&palette, &[Singer::Male, Singer::Female]);
        assert_eq!(text, "Male   Female");
        assert_eq!(colors.len(), text.chars().count());
        // First char of "Male" should be the male highlight color...
        assert_eq!(colors[0].r, palette.male_highlight.r);
        // ...and the first char of "Female" (after "Male" + 3 spaces) the
        // female highlight color.
        let female_start = "Male   ".chars().count();
        assert_eq!(colors[female_start].r, palette.female_highlight.r);
        assert_eq!(colors[female_start].g, palette.female_highlight.g);
    }

    #[test]
    fn lerp_interpolates_correctly() {
        let a = Rgb8::new(0, 0, 0);
        let b = Rgb8::new(200, 100, 50);
        let mid = a.lerp(b, 0.5);
        assert_eq!(mid.r, 100);
        assert_eq!(mid.g, 50);
        assert_eq!(mid.b, 25);
        assert_eq!(a.lerp(b, 0.0).r, 0);
        assert_eq!(a.lerp(b, 1.0).r, 200);
    }

    #[test]
    fn countdown_lit_count_fills_final_quarter_fully() {
        // Right up to (but not including) the transition, all 4 should be lit -
        // this is the fix for dot 4 previously only flashing for an instant.
        assert_eq!(countdown_lit_count(0.0, 4.0, 0.0), 1);
        assert_eq!(countdown_lit_count(0.0, 4.0, 0.99), 1);
        assert_eq!(countdown_lit_count(0.0, 4.0, 1.0), 2);
        assert_eq!(countdown_lit_count(0.0, 4.0, 2.0), 3);
        assert_eq!(countdown_lit_count(0.0, 4.0, 3.0), 4);
        assert_eq!(countdown_lit_count(0.0, 4.0, 3.99), 4);
    }

    #[test]
    fn blocks_split_on_explicit_block_boundaries() {
        // Simulates lyrics pasted as:
        //   a
        //   b
        //   c
        //   <blank line>
        //   d
        //   e
        let mut lines = vec![
            LyricLine::new("a"),
            LyricLine::new("b"),
            LyricLine::new("c"),
            LyricLine::new("d"),
            LyricLine::new("e"),
        ];
        lines[1].starts_new_block = false;
        lines[2].starts_new_block = false;
        // lines[3] ("d") keeps the default `true` - simulating the blank line before it.
        lines[4].starts_new_block = false;

        lines[0].start = Some(0.0);
        lines[1].start = Some(2.0);
        lines[2].start = Some(4.0);
        lines[3].start = Some(6.0); // note: no big timing gap here at all -
        lines[4].start = Some(8.0); // grouping should still split, since it's blank-line-driven now.
        let timed = resolve_timing(&lines, Some(10.0));
        let blocks = group_into_blocks(&timed);
        assert_eq!(blocks, vec![vec![0, 1, 2], vec![3, 4]]);
    }

    #[test]
    fn blocks_cap_at_max_lines() {
        let mut lines: Vec<LyricLine> = (0..8)
            .map(|i| LyricLine::new(format!("line {i}")))
            .collect();
        // Simulate one single unbroken run (no blank lines at all) - should
        // still get capped into chunks of at most MAX_BLOCK_LINES.
        for l in lines.iter_mut().skip(1) {
            l.starts_new_block = false;
        }
        for (i, l) in lines.iter_mut().enumerate() {
            l.start = Some(i as f64 * 2.0);
        }
        let timed = resolve_timing(&lines, Some(20.0));
        let blocks = group_into_blocks(&timed);
        for b in &blocks {
            assert!(b.len() <= MAX_BLOCK_LINES, "block too large: {b:?}");
        }
        // All 8 indices should still be covered, in order, no gaps/dupes.
        let flat: Vec<usize> = blocks.into_iter().flatten().collect();
        assert_eq!(flat, (0..8).collect::<Vec<_>>());
    }

    #[test]
    fn current_line_wipe_fraction_is_monotonic_non_decreasing() {
        let mut lines = vec![LyricLine::new("one two three four five")];
        lines[0].start = Some(0.0);
        let timed = resolve_timing(&lines, Some(10.0));
        let line = &timed[0];

        let mut last_frac = -1.0f32;
        for i in 0..=20 {
            let t = line.sing_end * (i as f64 / 20.0);
            let frac = current_line_wipe_fraction(line, t);
            assert!(
                frac >= last_frac,
                "wipe fraction should never decrease over time"
            );
            assert!(
                (0.0..=1.0).contains(&frac),
                "wipe fraction out of range: {frac}"
            );
            last_frac = frac;
        }
        // By sing_end, the wipe should have reached (or be very close to) the end.
        assert!(
            last_frac > 0.95,
            "expected wipe to be nearly/fully complete by sing_end, got {last_frac}"
        );
    }

    #[test]
    fn wipe_fraction_reaches_word_boundaries_at_word_start_times() {
        let mut lines = vec![LyricLine::new("aa bb cc")];
        lines[0].start = Some(0.0);
        let timed = resolve_timing(&lines, Some(10.0));
        let line = &timed[0];
        let words = word_timings(line);
        // At the exact moment the 2nd word starts, the wipe should have
        // reached exactly the char-fraction where the 2nd word begins.
        let frac_at_word2_start = current_line_wipe_fraction(line, words[1].highlight_at);
        let expected = 3.0 / 8.0; // "aa " is 3 chars out of "aa bb cc" (8 chars)
        assert!(
            (frac_at_word2_start - expected).abs() < 0.01,
            "got {frac_at_word2_start}, expected ~{expected}"
        );
    }

    #[test]
    fn render_video_smoke_test_with_blocks_and_intro_countdown() {
        // Just confirms the whole pipeline (blocks, intro countdown, wipe
        // rendering) runs without panicking across a realistic multi-line,
        // multi-block, duet song. Actual ffmpeg invocation is exercised
        // separately in manual testing since it needs a real audio file.
        let raw = "Siempre pasa\nCuando\nCierro los ojos\nNunca quiero";
        let mut lines = crate::lyrics::parse_pasted_lyrics(raw);
        let starts = [20.0, 22.5, 25.0, 27.5];
        for (l, s) in lines.iter_mut().zip(starts.iter()) {
            l.start = Some(*s);
        }
        lines[2].singer = Singer::Female;
        let timed = resolve_timing(&lines, Some(30.0));
        let blocks = group_into_blocks(&timed);
        assert_eq!(blocks, vec![vec![0, 1, 2, 3]]);

        let palette = VideoPalette {
            background: Rgb8::new(5, 5, 20),
            default_unsung: Rgb8::new(230, 230, 230),
            default_highlight: Rgb8::new(255, 220, 0),
            male_unsung: Rgb8::new(230, 230, 230),
            male_highlight: Rgb8::new(255, 220, 0),
            female_unsung: Rgb8::new(210, 210, 255),
            female_highlight: Rgb8::new(255, 90, 220),
            duet_unsung: Rgb8::new(200, 255, 200),
            duet_highlight: Rgb8::new(255, 150, 0),
            preview: Rgb8::new(110, 110, 160),
            title: Rgb8::new(255, 255, 255),
            artist: Rgb8::new(160, 160, 200),
            screaming_unsung: Rgb8::new(140, 30, 20),
            screaming_highlight: Rgb8::new(255, 60, 10),
        };
        let regular = FontArc::try_from_slice(DEJAVU_REGULAR).unwrap();
        let bold = FontArc::try_from_slice(DEJAVU_BOLD).unwrap();
        let card_end = crate::export::title_card_end(&timed); // long intro before first line at 20.0
        let mut canvas = Canvas::new(320, 180);

        // Sample across the whole song including the intro countdown window.
        let mut t = 0.0;
        while t < 30.0 {
            canvas.fill(palette.background);
            render_frame(
                &mut canvas,
                &regular,
                &bold,
                &timed,
                &blocks,
                &palette,
                Some("T"),
                Some("A"),
                card_end,
                t,
                &TimingSettings::default(),
            );
            t += 0.37;
        }
    }

    fn test_palette() -> VideoPalette {
        VideoPalette {
            background: Rgb8::new(5, 5, 20),
            default_unsung: Rgb8::new(230, 230, 230),
            default_highlight: Rgb8::new(255, 220, 0),
            male_unsung: Rgb8::new(230, 230, 230),
            male_highlight: Rgb8::new(255, 220, 0),
            female_unsung: Rgb8::new(210, 210, 255),
            female_highlight: Rgb8::new(255, 90, 220),
            duet_unsung: Rgb8::new(200, 255, 200),
            duet_highlight: Rgb8::new(255, 150, 0),
            preview: Rgb8::new(110, 110, 160),
            title: Rgb8::new(255, 255, 255),
            artist: Rgb8::new(160, 160, 200),
            screaming_unsung: Rgb8::new(140, 30, 20),
            screaming_highlight: Rgb8::new(255, 60, 10),
        }
    }

    /// True if the leftmost countdown dot's center pixel (see
    /// [`draw_countdown_dots`]'s geometry) is still exactly the background
    /// color - i.e. nothing was drawn there.
    fn countdown_dot_area_is_untouched(canvas: &Canvas, palette: &VideoPalette) -> bool {
        let w = canvas.w as f32;
        let h = canvas.h as f32;
        let cy = (h * 0.85).round() as usize;
        let cx = (w / 2.0 + (0.0 - 1.5) * (h * 0.05)).round() as usize;
        let idx = (cy * canvas.w + cx) * 3;
        canvas.buf[idx] == palette.background.r
            && canvas.buf[idx + 1] == palette.background.g
            && canvas.buf[idx + 2] == palette.background.b
    }

    /// True if any pixel in the horizontal row at `y` differs from the
    /// background color - i.e. something was actually drawn there.
    fn row_has_non_background_pixel(canvas: &Canvas, palette: &VideoPalette, y: f32) -> bool {
        let row = (y.round() as usize).min(canvas.h.saturating_sub(1));
        (0..canvas.w).any(|x| {
            let idx = (row * canvas.w + x) * 3;
            canvas.buf[idx] != palette.background.r
                || canvas.buf[idx + 1] != palette.background.g
                || canvas.buf[idx + 2] != palette.background.b
        })
    }

    #[test]
    fn backing_vocal_renders_only_during_its_own_window() {
        let mut lines = vec![LyricLine::new("hello world")];
        lines[0].start = Some(0.0);
        let mut bv = crate::lyrics::BackingVocal::new("hey");
        bv.start = Some(3.0);
        bv.end = Some(5.0);
        lines[0].backing_vocal = Some(bv);
        let timed = resolve_timing(&lines, Some(10.0));
        let blocks = group_into_blocks(&timed);
        assert!(timed[0].backing_vocal.is_some());

        let palette = test_palette();
        let regular = FontArc::try_from_slice(DEJAVU_REGULAR).unwrap();
        let bold = FontArc::try_from_slice(DEJAVU_BOLD).unwrap();
        let card_end = crate::export::title_card_end(&timed);
        let (w, h) = (320usize, 180usize);

        // Same geometry `render_frame` itself uses for a single-line block.
        let line_height = h as f32 * 0.11;
        let start_y = h as f32 * 0.5 - line_height / 2.0 + line_height * 0.5;
        let backing_y = start_y + line_height * 0.5;

        let render_at = |t: f64| {
            let mut canvas = Canvas::new(w, h);
            canvas.fill(palette.background);
            render_frame(
                &mut canvas,
                &regular,
                &bold,
                &timed,
                &blocks,
                &palette,
                None,
                None,
                card_end,
                t,
                &TimingSettings::default(),
            );
            canvas
        };

        // Before the backing vocal's own window (t=1.0, window is [3,5)) -
        // nothing at its row.
        let before = render_at(1.0);
        assert!(!row_has_non_background_pixel(&before, &palette, backing_y));

        // Inside its window - something's there.
        let during = render_at(4.0);
        assert!(row_has_non_background_pixel(&during, &palette, backing_y));

        // After its window (still within the host's own [0,10) window) -
        // gone again.
        let after = render_at(7.0);
        assert!(!row_has_non_background_pixel(&after, &palette, backing_y));
    }

    #[test]
    fn no_countdown_dots_after_the_last_line_even_with_a_long_silent_tail() {
        // Regression: a long stretch of silence after the *last* line (e.g.
        // a long instrumental outro) must not draw the countdown dots -
        // there's no next line to count into.
        let mut lines = vec![LyricLine::new("hi")];
        lines[0].start = Some(0.0);
        let timed = resolve_timing(&lines, Some(30.0)); // huge trailing silence
        let blocks = group_into_blocks(&timed);
        let palette = test_palette();
        let regular = FontArc::try_from_slice(DEJAVU_REGULAR).unwrap();
        let bold = FontArc::try_from_slice(DEJAVU_BOLD).unwrap();
        let card_end = crate::export::title_card_end(&timed);
        let mut canvas = Canvas::new(320, 180);
        canvas.fill(palette.background);

        // "hi" sings for ~1.2s, so with a 30s total duration the countdown
        // window (were it not suppressed) would be [26.0, 30.0) - sample
        // well inside that window, still within the line's own [0, 30) span.
        render_frame(
            &mut canvas,
            &regular,
            &bold,
            &timed,
            &blocks,
            &palette,
            None,
            None,
            card_end,
            28.0,
            &TimingSettings::default(),
        );

        assert!(
            countdown_dot_area_is_untouched(&canvas, &palette),
            "no countdown dot should be drawn after the last line"
        );
    }

    #[test]
    fn countdown_dots_still_show_for_a_real_mid_song_gap() {
        // Sanity check alongside the test above: only the end-of-song case
        // changed - a long gap *between* two real lines still draws dots.
        let mut lines = vec![LyricLine::new("hi"), LyricLine::new("there")];
        lines[0].start = Some(0.0);
        lines[1].start = Some(20.0); // huge mid-song gap
        let timed = resolve_timing(&lines, Some(22.0));
        let blocks = group_into_blocks(&timed);
        let palette = test_palette();
        let regular = FontArc::try_from_slice(DEJAVU_REGULAR).unwrap();
        let bold = FontArc::try_from_slice(DEJAVU_BOLD).unwrap();
        let card_end = crate::export::title_card_end(&timed);
        let mut canvas = Canvas::new(320, 180);
        canvas.fill(palette.background);

        render_frame(
            &mut canvas,
            &regular,
            &bold,
            &timed,
            &blocks,
            &palette,
            None,
            None,
            card_end,
            18.0,
            &TimingSettings::default(),
        );

        assert!(
            !countdown_dot_area_is_untouched(&canvas, &palette),
            "expected a countdown dot to be drawn for a real mid-song gap"
        );
    }
}
