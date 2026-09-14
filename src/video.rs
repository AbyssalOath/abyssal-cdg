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
//! This requires `ffmpeg` to be installed and on the PATH; we check for it
//! up front and return a clear error with install instructions if missing.

#[cfg(test)]
use crate::lyrics::MAX_BLOCK_LINES;
use crate::lyrics::{
    blank_sung_lines, countdown_window, countdown_window_between, current_line_wipe_fraction,
    group_into_blocks, hide_upcoming_lines, normalize_text, TimedLine,
};
use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use anyhow::{anyhow, bail, Context, Result};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

static DEJAVU_REGULAR: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");
static DEJAVU_BOLD: &[u8] = include_bytes!("../assets/DejaVuSans-Bold.ttf");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    fn singer_colors(&self, s: crate::lyrics::Singer) -> (Rgb8, Rgb8) {
        use crate::lyrics::Singer;
        match s {
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

fn measure_width(font: &FontRef, scale: PxScale, text: &str) -> f32 {
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
    font: &FontRef,
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
    font: &FontRef,
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

/// Convenience wrapper for a whole line in a single uniform color.
#[allow(clippy::too_many_arguments)]
fn draw_text_line_uniform(
    canvas: &mut Canvas,
    font: &FontRef,
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

#[allow(clippy::too_many_arguments)]
fn render_frame(
    canvas: &mut Canvas,
    regular: &FontRef,
    bold: &FontRef,
    timed_lines: &[TimedLine],
    blocks: &[Vec<usize>],
    palette: &VideoPalette,
    title: Option<&str>,
    artist: Option<&str>,
    card_end: f64,
    t: f64,
) {
    let w = canvas.w as f32;
    let h = canvas.h as f32;
    canvas.fill(palette.background);
    let max_width = w * 0.92;

    let has_title_card = title.is_some() || artist.is_some();
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
        return;
    }

    // Between the title card fading and the first line actually starting:
    // if that gap is long, show the same "get ready" countdown used for
    // mid-song breaks, instead of just sitting on a blank screen.
    if let Some(first) = timed_lines.first() {
        if t < first.start {
            if let Some((cd_start, cd_end)) = countdown_window_between(card_end, first.start) {
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
    let hide_upcoming = hide_upcoming_lines(&timed_lines[current_idx], t);
    let blank_sung = blank_sung_lines(&timed_lines[current_idx], t);

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

    if let Some((cd_start, cd_end)) = countdown_window(&timed_lines[current_idx]) {
        if t >= cd_start {
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

/// Confirms `ffmpeg` is installed and callable, with a clear, actionable
/// error message if not (video export needs it; the `.cdg` path doesn't).
pub fn check_ffmpeg_available() -> Result<()> {
    let result = Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match result {
        Ok(status) if status.success() => Ok(()),
        _ => bail!(
            "ffmpeg isn't installed (or isn't on your PATH). Video export needs it to encode \
             the MP4. Install it and try again:\n\
             - Windows: download from https://ffmpeg.org/download.html and add it to PATH\n\
             - macOS: brew install ffmpeg\n\
             - Linux: sudo apt install ffmpeg (or your distro's equivalent)"
        ),
    }
}

/// Renders the full karaoke video to `output_path`, muxed with the audio
/// at `audio_path`. Calls `on_progress(0.0..=1.0)` periodically so the UI
/// can show a progress bar - this can take anywhere from several seconds to
/// a few minutes depending on song length and resolution.
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
    output_path: &Path,
    mut on_progress: impl FnMut(f32),
) -> Result<()> {
    check_ffmpeg_available()?;

    let regular =
        FontRef::try_from_slice(DEJAVU_REGULAR).context("failed to parse embedded regular font")?;
    let bold =
        FontRef::try_from_slice(DEJAVU_BOLD).context("failed to parse embedded bold font")?;

    let (w, h) = resolution.dimensions();
    let card_end = crate::export::title_card_end(timed_lines);
    let blocks = group_into_blocks(timed_lines);
    let total_frames = ((total_duration * fps as f64).ceil() as u64).max(1);

    let mut child = Command::new("ffmpeg")
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
            "libx264",
            "-preset",
            "medium",
            "-crf",
            "18",
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
        );
        if stdin.write_all(&canvas.buf).is_err() {
            break;
        }
        if frame_idx % (fps as u64).max(1) == 0 {
            on_progress(frame_idx as f32 / total_frames as f32);
        }
    }
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
        let regular = FontRef::try_from_slice(DEJAVU_REGULAR).unwrap();
        let bold = FontRef::try_from_slice(DEJAVU_BOLD).unwrap();
        let card_end = crate::export::title_card_end(&timed); // long intro before first line at 20.0
        let mut canvas = Canvas::new(320, 180);

        // Sample across the whole song including the intro countdown window.
        let mut t = 0.0;
        while t < 30.0 {
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
            );
            t += 0.37;
        }
    }
}
