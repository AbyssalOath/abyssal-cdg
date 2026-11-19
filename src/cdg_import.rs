//! Reads line and word timing back out of an existing `.cdg` file, so a
//! song someone already timed (e.g. a censored release you want to redo
//! uncensored) doesn't have to be re-tapped and fine-tuned from scratch.
//!
//! A `.cdg` holds no text and no timestamps - only "paint this 6x12 tile"
//! and "change the palette" instructions, 300 a second. What it *does*
//! hold is the highlight wipe: while a line is sung, its text pixels are
//! repainted from the "unsung" color to the "sung" one, left to right. So:
//!
//! 1. [`extract_timing`] decodes the stream and notes, for every text
//!    pixel, when it last changed from one non-background color to another
//!    before being cleared - that's when the wipe reached it. Changes that
//!    involve the background (drawing text, erasing it) aren't wipes.
//! 2. Those pixel times are grouped into lines by following the wipe as it
//!    sweeps left to right along one band of the screen (a jump back to the
//!    left, a different band, or a long pause starts a new line).
//! 3. Within a line, pixel columns are split into words at the blank gaps
//!    between them; each word's time is when the wipe first touched it.
//!
//! There's no OCR: the *words* come from the user's own lyrics, matched to
//! the extracted lines by word count ([`apply_timing`]). That keeps it
//! reliable, and is exactly what's wanted for an uncensored re-do anyway.
//!
//! Tested against `.cdg`s from three different karaoke programs: XOR-tile
//! wipes, plain-tile wipes, smooth mid-word sweeps, letter-by-letter text
//! drawing and animated intro graphics. Scroll instructions are ignored.

use crate::cdg::{PACKETS_PER_SEC, PACKET_SIZE};
use crate::lyrics::{timing_units, LyricLine};

const WIDTH: usize = 300;
const HEIGHT: usize = 216;

const CMD_CDG: u8 = 0x09;
const INST_MEMORY_PRESET: u8 = 1;
const INST_TILE_BLOCK: u8 = 6;
const INST_TILE_BLOCK_XOR: u8 = 38;

/// A wiped pixel more than this far *left* of how far its line's wipe has
/// already got means a new line has started (lines can sit on adjacent
/// pixel rows with no blank row between them).
const LINE_RESET_PX: usize = 60;
/// A pause in the wipe longer than this (seconds) ends the line.
const LINE_PAUSE_SECS: f64 = 6.0;
/// How far above/below a line's pixel rows a wiped pixel can sit and still
/// belong to it.
const LINE_Y_SLACK: usize = 4;
/// A "line" taller than this many pixels is a picture, not text.
const MAX_LINE_HEIGHT: usize = 30;
/// Lines shorter than this (in wiped columns or seconds) are noise.
const MIN_LINE_COLUMNS: usize = 8;
const MIN_LINE_SECS: f64 = 0.25;
/// A word whose wipe finishes this much before the next word starts gets
/// an explicit end time, keeping the original's held pause.
const WORD_PAUSE_SECS: f64 = 0.3;

/// One sung line found in a `.cdg`: when the wipe reached each pixel
/// column of its text, and when the whole line finished. Words are found
/// from the blank gaps between columns - see [`CdgLine::words`].
#[derive(Clone, Debug, PartialEq)]
pub struct CdgLine {
    /// `(x, first wipe time)` per wiped pixel column, left to right.
    cols: Vec<(usize, f64)>,
    pub end: f64,
}

impl CdgLine {
    pub fn start(&self) -> f64 {
        self.cols.iter().map(|c| c.1).fold(self.end, f64::min)
    }

    /// The line's words - column runs separated by blank gaps of at least
    /// `gap` pixels - as `(start, end)`: when the wipe first reached the
    /// word, and when it finished it. How wide a word gap is depends on
    /// the font (2px letter gaps and 4px spaces in one file, 4px letter
    /// gaps and 13px spaces in another), so [`apply_timing`] picks `gap`
    /// per file.
    pub fn words(&self, gap: usize) -> Vec<(f64, f64)> {
        let mut words: Vec<(f64, f64)> = Vec::new();
        let mut prev_x: Option<usize> = None;
        for &(x, t) in &self.cols {
            match (prev_x, words.last_mut()) {
                (Some(px), Some(w)) if x - px < gap => {
                    w.0 = w.0.min(t);
                    w.1 = w.1.max(t);
                }
                _ => words.push((t, t)),
            }
            prev_x = Some(x);
        }
        // Keep word starts in order (a stray early pixel shouldn't reorder
        // a line).
        for k in 1..words.len() {
            words[k].0 = words[k].0.max(words[k - 1].0);
            words[k].1 = words[k].1.max(words[k].0);
        }
        words
    }
}

/// Word gaps (px) [`apply_timing`] tries; see [`CdgLine::words`].
const GAP_RANGE: std::ops::RangeInclusive<usize> = 2..=16;

/// When a pixel was wiped, and where.
#[derive(Clone, Copy)]
struct WipedPixel {
    t: f64,
    x: usize,
    y: usize,
}

/// Decodes `bytes` and returns every sung line found, in the order they're
/// sung.
pub fn extract_timing(bytes: &[u8]) -> Vec<CdgLine> {
    group_lines(wiped_pixels(bytes))
}

/// Step 1: decode, recording each pixel's last foreground-to-foreground
/// color change before it's cleared/redrawn (see the module docs).
fn wiped_pixels(bytes: &[u8]) -> Vec<WipedPixel> {
    let mut screen = vec![0u8; WIDTH * HEIGHT];
    let mut bg = 0u8;
    let mut pending: Vec<Option<f64>> = vec![None; WIDTH * HEIGHT];
    let mut out = Vec::new();

    let commit = |pending: &mut Vec<Option<f64>>, out: &mut Vec<WipedPixel>, i: usize| {
        if let Some(t) = pending[i].take() {
            out.push(WipedPixel {
                t,
                x: i % WIDTH,
                y: i / WIDTH,
            });
        }
    };

    for (n, packet) in bytes.as_chunks::<PACKET_SIZE>().0.iter().enumerate() {
        if packet[0] & 0x3F != CMD_CDG {
            continue;
        }
        let t = n as f64 / PACKETS_PER_SEC;
        let data = &packet[4..20];
        match packet[1] & 0x3F {
            INST_MEMORY_PRESET => {
                // Encoders repeat a preset up to 16 times (data[1] counts
                // them); only the first can change anything.
                if data[1] & 0x0F == 0 {
                    for i in 0..pending.len() {
                        commit(&mut pending, &mut out, i);
                    }
                }
                bg = data[0] & 0x0F;
                screen.fill(bg);
            }
            inst @ (INST_TILE_BLOCK | INST_TILE_BLOCK_XOR) => {
                let color0 = data[0] & 0x0F;
                let color1 = data[1] & 0x0F;
                let row = (data[2] & 0x1F) as usize;
                let col = (data[3] & 0x3F) as usize;
                let (x0, y0) = (col * 6, row * 12);
                if x0 + 6 > WIDTH || y0 + 12 > HEIGHT {
                    continue;
                }
                let xor = inst == INST_TILE_BLOCK_XOR;
                for (r, bits) in data[4..16].iter().enumerate() {
                    for c in 0..6 {
                        let i = (y0 + r) * WIDTH + x0 + c;
                        let color = if bits & (0x20 >> c) != 0 {
                            color1
                        } else {
                            color0
                        };
                        let old = screen[i];
                        let new = if xor { old ^ color } else { color };
                        if new == old {
                            continue;
                        }
                        screen[i] = new;
                        if old == bg || new == bg {
                            // Text drawn or erased: whatever wipe this
                            // pixel had belongs to the text that was here.
                            commit(&mut pending, &mut out, i);
                        } else {
                            // A color change on text: a wipe, unless a
                            // later one supersedes it. A plain tile
                            // repainting old text is a redraw for a new
                            // line, so the old line's wipe is kept first.
                            // (XOR changes overwrite instead: some encoders
                            // XOR an outline on while drawing, before the
                            // real wipe.)
                            if !xor {
                                commit(&mut pending, &mut out, i);
                            }
                            pending[i] = Some(t);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    for i in 0..pending.len() {
        commit(&mut pending, &mut out, i);
    }
    out
}

/// A line being assembled in [`group_lines`].
struct LineBuild {
    t0: f64,
    t1: f64,
    ymin: usize,
    ymax: usize,
    front: usize,
    /// First wipe time per pixel column, and the latest.
    cols: std::collections::BTreeMap<usize, (f64, f64)>,
}

/// Steps 2 and 3: wiped pixels -> lines of words.
fn group_lines(mut pixels: Vec<WipedPixel>) -> Vec<CdgLine> {
    pixels.sort_by(|a, b| a.t.total_cmp(&b.t).then(a.x.cmp(&b.x)));
    let mut lines: Vec<LineBuild> = Vec::new();
    let mut current: Option<usize> = None;
    for p in pixels {
        let fits = |l: &LineBuild| {
            p.y + LINE_Y_SLACK >= l.ymin
                && p.y <= l.ymax + LINE_Y_SLACK
                && p.t - l.t1 < LINE_PAUSE_SECS
                && p.x + LINE_RESET_PX >= l.front
                // Wiping a column this line already wiped (other than the
                // rest of the same stroke) means a new line in its place.
                && l.cols.get(&p.x).is_none_or(|c| p.t - c.0 < 1.0)
        };
        // The line the wipe was just on is the likeliest; a few recent ones
        // cover encoders that wipe two lines (e.g. a duet) interleaved.
        let target = current.filter(|&i| fits(&lines[i])).or_else(|| {
            (lines.len().saturating_sub(6)..lines.len())
                .rev()
                .find(|&i| fits(&lines[i]))
        });
        let i = match target {
            Some(i) => i,
            None => {
                lines.push(LineBuild {
                    t0: p.t,
                    t1: p.t,
                    ymin: p.y,
                    ymax: p.y,
                    front: p.x,
                    cols: Default::default(),
                });
                lines.len() - 1
            }
        };
        current = Some(i);
        let l = &mut lines[i];
        l.t1 = l.t1.max(p.t);
        l.ymin = l.ymin.min(p.y);
        l.ymax = l.ymax.max(p.y);
        l.front = l.front.max(p.x);
        let e = l.cols.entry(p.x).or_insert((p.t, p.t));
        e.0 = e.0.min(p.t);
        e.1 = e.1.max(p.t);
    }

    let mut out: Vec<CdgLine> = lines
        .into_iter()
        .filter(|l| {
            l.ymax - l.ymin <= MAX_LINE_HEIGHT
                && l.cols.len() >= MIN_LINE_COLUMNS
                && l.t1 - l.t0 >= MIN_LINE_SECS
        })
        .map(|l| CdgLine {
            cols: l.cols.iter().map(|(&x, &(first, _))| (x, first)).collect(),
            end: l.t1,
        })
        .filter(|l| !is_redraw_burst(l) && !is_unordered(l))
        .collect();
    out.sort_by(|a, b| a.start().total_cmp(&b.start()));
    out
}

/// Text recolored all at once (an encoder redrawing a new line over the
/// old one, or a whole sung line changing color when it's done) rather
/// than wiped: most of a wide line changes within a fifth of a second.
fn is_redraw_burst(l: &CdgLine) -> bool {
    if l.cols.len() < 40 {
        return false;
    }
    let mut ts: Vec<f64> = l.cols.iter().map(|c| c.1).collect();
    ts.sort_by(f64::total_cmp);
    let p10 = ts[ts.len() / 10];
    let p90 = ts[ts.len() * 9 / 10];
    p90 - p10 < 0.2
}

/// An animation or picture: a real wipe runs left to right, so most
/// columns are reached no earlier than the ones to their left.
fn is_unordered(l: &CdgLine) -> bool {
    let mut latest = f64::NEG_INFINITY;
    let mut behind = 0;
    for &(_, t) in &l.cols {
        if t < latest - 0.3 {
            behind += 1;
        }
        latest = latest.max(t);
    }
    behind * 3 > l.cols.len()
}

/// How [`apply_timing`] went, for the status line.
#[derive(Debug, Default, PartialEq)]
pub struct ImportReport {
    /// Lines given full word timing (word counts matched).
    pub exact: usize,
    /// Lines given a start/end but estimated word timing (word counts
    /// differed - e.g. a changed lyric).
    pub line_only: usize,
    /// Lines with no counterpart in the `.cdg` (left untimed).
    pub unmatched: usize,
    /// `.cdg` lines that matched none of the lyrics (titles, "Intro...",
    /// lines that were removed).
    pub cdg_unused: usize,
}

impl ImportReport {
    pub fn summarize(&self) -> String {
        let mut s = format!(
            "Imported .cdg timing: {} line(s) with word timing",
            self.exact
        );
        if self.line_only > 0 {
            s.push_str(&format!(
                ", {} with line timing only (word count differs from the .cdg - shown \
                 grey; Auto-align or click their words)",
                self.line_only
            ));
        }
        if self.unmatched > 0 {
            s.push_str(&format!(
                ", {} not found in the .cdg (left untimed - tap them)",
                self.unmatched
            ));
        }
        s.push('.');
        if self.cdg_unused > 0 {
            s.push_str(&format!(
                " {} .cdg line(s) went unused (titles, intro text or removed lines).",
                self.cdg_unused
            ));
        }
        s.push_str(" Play it through to check. Ctrl+Z undoes the import.");
        s
    }
}

/// One step of the line alignment in [`align`].
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// Lyric lines `[i, i+a)` match `.cdg` lines `[j, j+b)` (a, b in 1..=2,
    /// not both 2).
    Match(usize, usize),
    SkipLyric,
    SkipCdg,
}

/// Lines a lyric word count and a `.cdg` word count are apart, as a cost.
fn count_cost(a: usize, b: usize) -> f64 {
    a.abs_diff(b) as f64
}

/// Aligns lyric lines (by word count) to `.cdg` lines (by word count), in
/// order. Allows one lyric line to span two `.cdg` lines and vice versa -
/// the user's line breaks needn't match the original's - plus skipping
/// either side (added/removed lines, titles, intro text).
fn align(lyric: &[usize], cdg: &[usize]) -> (Vec<Step>, f64) {
    let (n, m) = (lyric.len(), cdg.len());
    let skip_lyric = |w: usize| 2.0 + 0.5 * w as f64;
    let skip_cdg = |w: usize| 1.0 + 0.3 * w as f64;
    let mut cost = vec![vec![f64::INFINITY; m + 1]; n + 1];
    let mut back = vec![vec![Step::SkipLyric; m + 1]; n + 1];
    cost[0][0] = 0.0;
    for i in 0..=n {
        for j in 0..=m {
            let here = cost[i][j];
            if !here.is_finite() {
                continue;
            }
            let mut relax = |ni: usize, nj: usize, c: f64, s: Step, cost: &mut Vec<Vec<f64>>| {
                if here + c < cost[ni][nj] {
                    cost[ni][nj] = here + c;
                    back[ni][nj] = s;
                }
            };
            if i < n {
                relax(i + 1, j, skip_lyric(lyric[i]), Step::SkipLyric, &mut cost);
            }
            if j < m {
                relax(i, j + 1, skip_cdg(cdg[j]), Step::SkipCdg, &mut cost);
            }
            for (a, b) in [(1, 1), (1, 2), (2, 1)] {
                if i + a <= n && j + b <= m {
                    let lw: usize = lyric[i..i + a].iter().sum();
                    let cw: usize = cdg[j..j + b].iter().sum();
                    let split = if a + b > 2 { 0.5 } else { 0.0 };
                    relax(
                        i + a,
                        j + b,
                        count_cost(lw, cw) + split,
                        Step::Match(a, b),
                        &mut cost,
                    );
                }
            }
        }
    }
    let mut steps = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        let s = back[i][j];
        steps.push(s);
        match s {
            Step::Match(a, b) => {
                i -= a;
                j -= b;
            }
            Step::SkipLyric => i -= 1,
            Step::SkipCdg => j -= 1,
        }
    }
    steps.reverse();
    (steps, cost[n][m])
}

/// How many lyric lines `steps` matches word for word.
fn exact_lines(steps: &[Step], lyric: &[usize], cdg: &[usize]) -> usize {
    let (mut i, mut j, mut exact) = (0, 0, 0);
    for s in steps {
        match *s {
            Step::Match(a, b) => {
                let lw: usize = lyric[i..i + a].iter().sum();
                let cw: usize = cdg[j..j + b].iter().sum();
                if lw == cw {
                    exact += a;
                }
                i += a;
                j += b;
            }
            Step::SkipLyric => i += 1,
            Step::SkipCdg => j += 1,
        }
    }
    exact
}

/// Whitespace-separated word count (what a `.cdg`'s gaps can see - syllable
/// marks are invisible on screen).
fn screen_words(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Sets `line`'s timing from `words` (one `(start, end)` per screen word,
/// already matched by count) and the sung end.
fn set_line_timing(line: &mut LyricLine, words: &[(f64, f64)], end: f64) {
    let units = timing_units(&line.text);
    line.word_overrides = vec![None; units.len()];
    line.word_end_overrides = vec![None; units.len()];
    // First unit of each screen word gets that word's time; later
    // syllables of the same word stay on the automatic estimate.
    let mut word = 0;
    let mut first_unit_of_word = true;
    let mut last_unit_of: Vec<usize> = Vec::new();
    for (k, (_, ends_word)) in units.iter().enumerate() {
        if first_unit_of_word {
            line.word_overrides[k] = words.get(word).map(|w| w.0);
        }
        first_unit_of_word = *ends_word;
        if *ends_word {
            last_unit_of.push(k);
            word += 1;
        }
    }
    // Keep the original's pauses: a word whose wipe finishes well before
    // the next one starts holds there instead of stretching to meet it.
    for (w, &k) in last_unit_of.iter().enumerate() {
        let Some(&(_, wend)) = words.get(w) else {
            break;
        };
        let next = words.get(w + 1).map_or(end, |n| n.0);
        if next - wend > WORD_PAUSE_SECS {
            line.word_end_overrides[k] = Some(wend);
        }
    }
    line.start = words.first().map(|w| w.0);
    line.sing_end_override = Some(end);
    line.estimated_sing_duration = None;
}

/// Times `lines` from `cdg` (see [`extract_timing`]): lines are matched by
/// word count, in order, tolerating different line breaks and added,
/// removed or reworded lines. A matched line whose word count agrees gets
/// full word timing; one that doesn't gets just its start and end (its
/// words fall back to the estimate). Unmatched lines are left untimed.
pub fn apply_timing(lines: &mut [LyricLine], cdg: &[CdgLine]) -> ImportReport {
    let lyric_counts: Vec<usize> = lines.iter().map(|l| screen_words(&l.text)).collect();
    // The word gap that lines the .cdg up best with the lyrics: most lines
    // matched word for word, then least total mismatch.
    let (cdg_words, steps) = GAP_RANGE
        .map(|gap| {
            let words: Vec<Vec<(f64, f64)>> = cdg.iter().map(|l| l.words(gap)).collect();
            let counts: Vec<usize> = words.iter().map(Vec::len).collect();
            let (steps, cost) = align(&lyric_counts, &counts);
            let exact = exact_lines(&steps, &lyric_counts, &counts);
            (words, steps, exact, cost)
        })
        .max_by(|a, b| a.2.cmp(&b.2).then(b.3.total_cmp(&a.3)))
        .map(|(w, s, _, _)| (w, s))
        .unwrap_or_default();
    let mut report = ImportReport::default();
    let (mut i, mut j) = (0, 0);
    for step in steps {
        match step {
            Step::SkipLyric => {
                let l = &mut lines[i];
                l.start = None;
                l.sing_end_override = None;
                l.estimated_sing_duration = None;
                l.word_overrides.iter_mut().for_each(|w| *w = None);
                l.word_end_overrides.iter_mut().for_each(|w| *w = None);
                report.unmatched += 1;
                i += 1;
            }
            Step::SkipCdg => {
                report.cdg_unused += 1;
                j += 1;
            }
            Step::Match(a, b) => {
                // Pool the matched .cdg lines' words, then deal them out
                // to the lyric line(s) in order.
                let pooled: Vec<(f64, f64)> =
                    cdg_words[j..j + b].iter().flatten().copied().collect();
                let cdg_end = cdg[j + b - 1].end;
                let wanted: usize = lyric_counts[i..i + a].iter().sum();
                if wanted == pooled.len() {
                    let mut at = 0;
                    for k in 0..a {
                        let n = lyric_counts[i + k];
                        let words = &pooled[at..at + n];
                        // A line's end: its own last .cdg line's end if it
                        // finishes one, else where its last word finished.
                        let end = if k + 1 == a {
                            cdg_end
                        } else {
                            words.last().map_or(cdg_end, |w| w.1)
                        };
                        set_line_timing(&mut lines[i + k], words, end);
                        at += n;
                        report.exact += 1;
                    }
                } else {
                    // Counts differ: give each lyric line the matching
                    // share of the .cdg span, word timing estimated.
                    let start = pooled.first().map_or(cdg[j].start(), |w| w.0);
                    let span = (cdg_end - start).max(0.0);
                    let mut at = 0usize;
                    for k in 0..a {
                        let n = lyric_counts[i + k];
                        let frac0 = at as f64 / wanted.max(1) as f64;
                        let frac1 = (at + n) as f64 / wanted.max(1) as f64;
                        let l = &mut lines[i + k];
                        set_line_timing(l, &[], 0.0);
                        l.start = Some(start + span * frac0);
                        l.sing_end_override = Some(start + span * frac1);
                        at += n;
                        report.line_only += 1;
                    }
                }
                i += a;
                j += b;
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::LyricLine;

    /// A tiny `.cdg` writer for tests: packets at explicit times.
    struct Stream(Vec<[u8; PACKET_SIZE]>);

    impl Stream {
        fn new() -> Self {
            Stream(Vec::new())
        }
        fn at(&mut self, secs: f64, inst: u8, data: [u8; 16]) {
            // Packets queue up one after another from `secs` on, like an
            // encoder spreading a burst of tiles over consecutive packets.
            let mut n = (secs * PACKETS_PER_SEC).round() as usize;
            while self.0.get(n).is_some_and(|p| p[0] != 0) {
                n += 1;
            }
            if self.0.len() <= n {
                self.0.resize(n + 1, [0; PACKET_SIZE]);
            }
            let mut p = [0u8; PACKET_SIZE];
            p[0] = CMD_CDG;
            p[1] = inst;
            p[4..20].copy_from_slice(&data);
            self.0[n] = p;
        }
        fn tile(&mut self, secs: f64, xor: bool, c1: u8, row: u8, col: u8, bits: u8) {
            let mut d = [0u8; 16];
            d[1] = c1;
            d[2] = row;
            d[3] = col;
            for r in 0..12 {
                d[4 + r] = bits;
            }
            self.at(
                secs,
                if xor {
                    INST_TILE_BLOCK_XOR
                } else {
                    INST_TILE_BLOCK
                },
                d,
            );
        }
        fn bytes(&self) -> Vec<u8> {
            self.0.concat()
        }
    }

    /// Draws `words` (each `cols` tiles wide of solid "letters", one blank
    /// tile between words) on tile `row`, then XOR-wipes each word tile by
    /// tile starting at its time. Returns the stream.
    fn wiped_line(s: &mut Stream, row: u8, draw_at: f64, words: &[(f64, u8)]) {
        // Text color 1 on background 0, letters 4px wide (0b111100) so
        // columns inside a word have 2px gaps (< word gap).
        let mut col = 1u8;
        let mut layout = Vec::new();
        for &(t, n) in words {
            layout.push((t, col, n));
            for k in 0..n {
                s.tile(draw_at, false, 1, row, col + k, 0b111100);
            }
            col += n + 1;
        }
        for (t, c, n) in layout {
            for k in 0..n {
                // 1 ^ 3 = 2: text color 1 -> sung color 2.
                s.tile(t + 0.1 * k as f64, true, 3, row, c + k, 0b111100);
            }
        }
    }

    #[test]
    fn extracts_word_starts_and_line_end() {
        let mut s = Stream::new();
        s.at(0.0, INST_MEMORY_PRESET, [0; 16]);
        wiped_line(&mut s, 5, 1.0, &[(2.0, 2), (3.0, 3), (4.5, 1)]);
        let lines = extract_timing(&s.bytes());
        assert_eq!(lines.len(), 1);
        let starts: Vec<f64> = lines[0].words(5).iter().map(|w| w.0).collect();
        assert_eq!(starts.len(), 3);
        for (got, want) in starts.iter().zip([2.0, 3.0, 4.5]) {
            assert!((got - want).abs() < 0.01, "{starts:?}");
        }
        assert!((lines[0].end - 4.5).abs() < 0.02);
    }

    #[test]
    fn text_drawing_alone_is_not_a_wipe() {
        let mut s = Stream::new();
        s.at(0.0, INST_MEMORY_PRESET, [0; 16]);
        wiped_line(&mut s, 5, 1.0, &[]);
        for c in 0..10 {
            s.tile(1.0, false, 1, 5, c, 0b111100);
        }
        assert!(extract_timing(&s.bytes()).is_empty());
    }

    #[test]
    fn redrawn_row_keeps_both_lines() {
        // Two lines sung on the same row, the second drawn over the first
        // with plain tiles (no screen clear in between).
        let mut s = Stream::new();
        s.at(0.0, INST_MEMORY_PRESET, [0; 16]);
        wiped_line(&mut s, 5, 1.0, &[(2.0, 2), (3.0, 2)]);
        wiped_line(&mut s, 5, 8.0, &[(9.0, 2), (10.0, 2), (11.0, 2)]);
        let lines = extract_timing(&s.bytes());
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0].words(5).len(), 2);
        assert_eq!(lines[1].words(5).len(), 3);
        assert!((lines[1].start() - 9.0).abs() < 0.01);
    }

    #[test]
    fn adjacent_rows_split_on_wipe_reset() {
        let mut s = Stream::new();
        s.at(0.0, INST_MEMORY_PRESET, [0; 16]);
        // Long first line so the second line's wipe jumps back left.
        wiped_line(&mut s, 5, 1.0, &[(2.0, 4), (3.0, 4), (4.0, 4), (5.0, 4)]);
        wiped_line(&mut s, 6, 1.0, &[(7.0, 2), (8.0, 2)]);
        let lines = extract_timing(&s.bytes());
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0].words(5).len(), 4);
        assert_eq!(lines[1].words(5).len(), 2);
    }

    /// A line whose words are 3px-wide column runs 20px apart (wider
    /// than any gap [`apply_timing`] tries), wiped from `start` to `end`.
    fn cdg_words(words: &[(f64, f64)], end: f64) -> CdgLine {
        let mut cols = Vec::new();
        for (k, &(s, e)) in words.iter().enumerate() {
            for c in 0..3 {
                cols.push((k * 20 + c, s + (e - s) * c as f64 / 2.0));
            }
        }
        CdgLine { cols, end }
    }

    fn cdg_line(starts: &[f64], end: f64) -> CdgLine {
        let words: Vec<(f64, f64)> = starts.iter().map(|&s| (s, s)).collect();
        cdg_words(&words, end)
    }

    #[test]
    fn exact_match_sets_word_timing() {
        let mut lines = vec![LyricLine::new("one two three"), LyricLine::new("four five")];
        let cdg = vec![cdg_line(&[1.0, 2.0, 3.0], 4.0), cdg_line(&[5.0, 6.0], 7.0)];
        let r = apply_timing(&mut lines, &cdg);
        assert_eq!(r.exact, 2);
        assert_eq!(lines[0].start, Some(1.0));
        assert_eq!(
            lines[0].word_overrides,
            vec![Some(1.0), Some(2.0), Some(3.0)]
        );
        assert_eq!(lines[0].sing_end_override, Some(4.0));
        assert_eq!(lines[1].word_overrides, vec![Some(5.0), Some(6.0)]);
    }

    #[test]
    fn title_lines_and_different_line_breaks() {
        // .cdg has a title line first, and splits what the user wrote as
        // one line into two.
        let mut lines = vec![LyricLine::new("a b c d e"), LyricLine::new("f g")];
        let cdg = vec![
            cdg_line(&[0.5], 0.9), // "Intro..."
            cdg_line(&[1.0, 2.0, 3.0], 3.5),
            cdg_line(&[4.0, 5.0], 5.5),
            cdg_line(&[6.0, 7.0], 8.0),
        ];
        let r = apply_timing(&mut lines, &cdg);
        assert_eq!(r.cdg_unused, 1);
        assert_eq!(r.exact, 2);
        assert_eq!(
            lines[0].word_overrides,
            vec![Some(1.0), Some(2.0), Some(3.0), Some(4.0), Some(5.0)]
        );
        assert_eq!(lines[1].start, Some(6.0));
    }

    #[test]
    fn reworded_line_gets_line_timing_only() {
        let mut lines = vec![
            LyricLine::new("x y"),
            LyricLine::new("now with two extra words"),
            LyricLine::new("z w"),
        ];
        let cdg = vec![
            cdg_line(&[1.0, 2.0], 3.0),
            cdg_line(&[4.0, 5.0, 6.0], 7.0),
            cdg_line(&[8.0, 9.0], 10.0),
        ];
        let r = apply_timing(&mut lines, &cdg);
        assert_eq!((r.exact, r.line_only, r.unmatched), (2, 1, 0));
        assert_eq!(lines[1].start, Some(4.0));
        assert_eq!(lines[1].sing_end_override, Some(7.0));
        assert!(lines[1].word_overrides.iter().all(Option::is_none));
        assert_eq!(lines[2].start, Some(8.0));
    }

    #[test]
    fn syllables_get_their_word_time_on_the_first_unit() {
        let mut lines = vec![LyricLine::new("beau|ti|ful day")];
        let cdg = vec![cdg_line(&[1.0, 3.0], 4.0)];
        apply_timing(&mut lines, &cdg);
        assert_eq!(
            lines[0].word_overrides,
            vec![Some(1.0), None, None, Some(3.0)]
        );
    }

    #[test]
    fn word_pause_is_kept() {
        let mut lines = vec![LyricLine::new("hold on")];
        let cdg = vec![cdg_words(&[(1.0, 1.5), (3.0, 3.4)], 3.5)];
        apply_timing(&mut lines, &cdg);
        assert_eq!(lines[0].word_end_overrides, vec![Some(1.5), None]);
    }

    /// Round trip through this app's own `.cdg` encoder: a song exported
    /// with known word timing should import back with the same timing.
    #[test]
    fn round_trips_this_apps_own_cdg() {
        let mut lines = vec![
            LyricLine::new("hello there my friend"),
            LyricLine::new("sing along with me"),
        ];
        let times = [[2.0, 2.6, 3.1, 3.5], [5.0, 5.7, 6.2, 6.9]];
        for (l, t) in lines.iter_mut().zip(times) {
            l.start = Some(t[0]);
            l.word_overrides = t.iter().map(|&x| Some(x)).collect();
            l.sing_end_override = Some(t[3] + 0.6);
        }
        let settings = crate::lyrics::TimingSettings::default();
        let timed = crate::lyrics::resolve_timing_with_settings(&lines, Some(12.0), &settings);
        let bytes =
            crate::export::render_cdg(&timed, 12.0, &crate::export::Palette::default(), None, None);
        if let Ok(p) = std::env::var("CDG_DUMP") {
            std::fs::write(p, &bytes).unwrap();
        }
        let cdg = extract_timing(&bytes);
        let mut fresh = vec![
            LyricLine::new("hello there my friend"),
            LyricLine::new("sing along with me"),
        ];
        let r = apply_timing(&mut fresh, &cdg);
        assert_eq!(r.exact, 2, "{cdg:?}");
        for (l, t) in fresh.iter().zip(times) {
            for (got, want) in l.word_overrides.iter().zip(t) {
                let got = got.expect("word timed");
                // One CDG packet is 1/300 s; the encoder's wipe steps are
                // coarser than that, so allow a small tolerance.
                assert!((got - want).abs() < 0.15, "got {got}, want {want}");
            }
        }
    }
}

/// Manual check against real files: `CDG_SAMPLE=path.cdg cargo test --release
/// dump_cdg_sample -- --ignored --nocapture` prints every line found, with
/// its word count at a few gap widths.
#[cfg(test)]
#[test]
#[ignore]
fn dump_cdg_sample() {
    let path = std::env::var("CDG_SAMPLE").expect("set CDG_SAMPLE");
    let bytes = std::fs::read(path).unwrap();
    for l in extract_timing(&bytes) {
        let counts: Vec<String> = [3, 5, 8, 12]
            .iter()
            .map(|&g| l.words(g).len().to_string())
            .collect();
        let starts: Vec<String> = l.words(5).iter().map(|w| format!("{:.2}", w.0)).collect();
        println!(
            "{:7.2}-{:7.2} words@3/5/8/12={} | {}",
            l.start(),
            l.end,
            counts.join("/"),
            starts.join(" ")
        );
        if std::env::var("CDG_GAPS").is_ok() {
            let gaps: Vec<String> = l
                .cols
                .windows(2)
                .filter(|w| w[1].0 - w[0].0 > 1)
                .map(|w| format!("{}@{:.2}", w[1].0 - w[0].0, w[1].1))
                .collect();
            println!("    gaps {}", gaps.join(" "));
        }
    }
}
