//! A small CD+Graphics *decoder*, for the live preview's ".cdg" mode: it
//! plays back the exact bytes `export::render_cdg_with_settings` would
//! write to the `.cdg` file, so what you see is what a karaoke player will
//! show - the 300x216 canvas, 16-color palette, 2x/1x text sizing and any
//! clipping - rather than the video-style preview's approximation.
//!
//! Supports the instructions this app's encoder emits (memory preset,
//! border preset, tile block, tile block XOR, load color table low/high).
//! Scroll instructions are ignored - nothing here writes them.
//!
//! [`CdgPlayback`] keeps a decoder snapshot every [`CHECKPOINT_PACKETS`]
//! packets, so jumping to any time (seeking, scrubbing backwards) decodes
//! at most a few seconds of packets instead of the whole song.

use crate::cdg::{PACKETS_PER_SEC, PACKET_SIZE};

pub const WIDTH: usize = 300;
pub const HEIGHT: usize = 216;

const CMD_CDG: u8 = 0x09;
const INST_MEMORY_PRESET: u8 = 1;
const INST_BORDER_PRESET: u8 = 2;
const INST_TILE_BLOCK: u8 = 6;
const INST_TILE_BLOCK_XOR: u8 = 38;
const INST_LOAD_CLUT_LOW: u8 = 30;
const INST_LOAD_CLUT_HIGH: u8 = 31;

/// Decoder snapshot spacing: 5 seconds of packets. A snapshot is ~65KB, so
/// a 4-minute song keeps ~3MB of them.
const CHECKPOINT_PACKETS: usize = 1500;

/// The CDG screen state: one palette index per pixel plus the 16-entry
/// palette (as 8-bit RGB, expanded from CDG's 4-bit channels).
#[derive(Clone)]
pub struct CdgDecoder {
    pixels: Vec<u8>,
    palette: [[u8; 3]; 16],
}

impl Default for CdgDecoder {
    fn default() -> Self {
        Self {
            pixels: vec![0; WIDTH * HEIGHT],
            palette: [[0; 3]; 16],
        }
    }
}

impl CdgDecoder {
    /// Applies one 24-byte packet. Non-CDG and unknown packets are no-ops.
    pub fn apply(&mut self, packet: &[u8]) {
        if packet.len() < PACKET_SIZE || packet[0] & 0x3F != CMD_CDG {
            return;
        }
        let data = &packet[4..20];
        match packet[1] & 0x3F {
            INST_MEMORY_PRESET => self.pixels.fill(data[0] & 0x0F),
            INST_BORDER_PRESET => {
                let c = data[0] & 0x0F;
                for y in 0..HEIGHT {
                    for x in 0..WIDTH {
                        if !(6..WIDTH - 6).contains(&x) || !(12..HEIGHT - 12).contains(&y) {
                            self.pixels[y * WIDTH + x] = c;
                        }
                    }
                }
            }
            inst @ (INST_TILE_BLOCK | INST_TILE_BLOCK_XOR) => {
                let color0 = data[0] & 0x0F;
                let color1 = data[1] & 0x0F;
                let row = (data[2] & 0x1F) as usize;
                let col = (data[3] & 0x3F) as usize;
                let (x0, y0) = (col * 6, row * 12);
                if x0 + 6 > WIDTH || y0 + 12 > HEIGHT {
                    return;
                }
                for (r, bits) in data[4..16].iter().enumerate() {
                    for c in 0..6 {
                        let on = bits & (0x20 >> c) != 0;
                        let color = if on { color1 } else { color0 };
                        let px = &mut self.pixels[(y0 + r) * WIDTH + x0 + c];
                        if inst == INST_TILE_BLOCK_XOR {
                            *px ^= color;
                        } else {
                            *px = color;
                        }
                    }
                }
            }
            inst @ (INST_LOAD_CLUT_LOW | INST_LOAD_CLUT_HIGH) => {
                let base = if inst == INST_LOAD_CLUT_HIGH { 8 } else { 0 };
                for i in 0..8 {
                    let b1 = data[i * 2] & 0x3F;
                    let b2 = data[i * 2 + 1] & 0x3F;
                    let r = (b1 >> 2) & 0x0F;
                    let g = ((b1 & 0x03) << 2) | ((b2 >> 4) & 0x03);
                    let b = b2 & 0x0F;
                    self.palette[base + i] = [r * 17, g * 17, b * 17];
                }
            }
            _ => {}
        }
    }

    /// The current screen as RGBA8, row-major, `WIDTH * HEIGHT * 4` bytes.
    pub fn rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(WIDTH * HEIGHT * 4);
        for &i in &self.pixels {
            let [r, g, b] = self.palette[i as usize & 0x0F];
            out.extend_from_slice(&[r, g, b, 255]);
        }
        out
    }

    #[cfg(test)]
    fn pixel(&self, x: usize, y: usize) -> u8 {
        self.pixels[y * WIDTH + x]
    }
}

/// A rendered `.cdg` stream plus checkpoints, for showing the screen at
/// any playback time.
pub struct CdgPlayback {
    bytes: Vec<u8>,
    /// `checkpoints[k]` = decoder state after `k * CHECKPOINT_PACKETS` packets.
    checkpoints: Vec<CdgDecoder>,
    decoder: CdgDecoder,
    /// Packets applied to `decoder` so far.
    pos: usize,
}

impl CdgPlayback {
    /// Decodes `bytes` once end to end to build the checkpoints - meant to
    /// run on a background thread along with rendering the bytes.
    pub fn new(bytes: Vec<u8>) -> Self {
        let mut decoder = CdgDecoder::default();
        let mut checkpoints = vec![decoder.clone()];
        for (i, packet) in bytes.as_chunks::<PACKET_SIZE>().0.iter().enumerate() {
            decoder.apply(packet);
            if (i + 1) % CHECKPOINT_PACKETS == 0 {
                checkpoints.push(decoder.clone());
            }
        }
        Self {
            bytes,
            decoder: checkpoints[0].clone(),
            checkpoints,
            pos: 0,
        }
    }

    fn packet_count(&self) -> usize {
        self.bytes.len() / PACKET_SIZE
    }

    /// Advances (or rewinds, via the nearest checkpoint) to the screen as
    /// it is at `t` seconds - every packet *before* `t * 300` applied, the
    /// same moment a player would be showing. Returns the packet position,
    /// which callers can compare to skip re-uploading an unchanged frame.
    pub fn seek(&mut self, t: f64) -> usize {
        let target = ((t.max(0.0) * PACKETS_PER_SEC) as usize).min(self.packet_count());
        if target < self.pos || target - self.pos > CHECKPOINT_PACKETS {
            let k = (target / CHECKPOINT_PACKETS).min(self.checkpoints.len() - 1);
            self.decoder = self.checkpoints[k].clone();
            self.pos = k * CHECKPOINT_PACKETS;
        }
        for packet in self.bytes[self.pos * PACKET_SIZE..target * PACKET_SIZE]
            .as_chunks::<PACKET_SIZE>()
            .0
        {
            self.decoder.apply(packet);
        }
        self.pos = target;
        self.pos
    }

    pub fn rgba(&self) -> Vec<u8> {
        self.decoder.rgba()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdg::{CdgColor, CdgWriter, SAFE_COL_OFFSET, SAFE_ROW_OFFSET};

    #[test]
    fn decodes_palette_preset_and_tiles_written_by_our_encoder() {
        let mut w = CdgWriter::new();
        let mut low = [CdgColor::new(0, 0, 0); 8];
        low[1] = CdgColor::new(15, 8, 3);
        w.load_color_table(&low, false);
        w.memory_preset(0);
        let mut tile = [0u8; 12];
        tile[0] = 0b10_0000; // leftmost pixel of the top row
        w.tile_block(0, 0, 0, 1, &tile);
        let mut d = CdgDecoder::default();
        for p in w.into_bytes().as_chunks::<PACKET_SIZE>().0 {
            d.apply(p);
        }
        assert_eq!(d.palette[1], [255, 136, 51]);
        let (x, y) = (SAFE_COL_OFFSET as usize * 6, SAFE_ROW_OFFSET as usize * 12);
        assert_eq!(d.pixel(x, y), 1);
        assert_eq!(d.pixel(x + 1, y), 0);
        assert_eq!(d.pixel(x, y + 1), 0);
    }

    #[test]
    fn a_real_export_shows_lyrics_only_once_their_line_is_up() {
        use crate::export::{render_cdg, Palette};
        use crate::lyrics::{Singer, TimedLine};
        let lines = vec![TimedLine::new(
            "Hello world".to_string(),
            8.0,
            12.0,
            Singer::Default,
        )];
        let bytes = render_cdg(&lines, 15.0, &Palette::default(), None, None);
        let mut pb = CdgPlayback::new(bytes);
        let lit = |pb: &mut CdgPlayback, t: f64| {
            pb.seek(t);
            let bg = pb.decoder.pixel(WIDTH / 2, 2);
            pb.decoder.pixels.iter().filter(|&&p| p != bg).count()
        };
        assert_eq!(lit(&mut pb, 0.5), 0, "nothing drawn before the line");
        assert!(lit(&mut pb, 9.0) > 50, "line text drawn while it's sung");
    }

    #[test]
    fn syllable_marks_are_never_drawn_and_syllables_wipe_separately() {
        use crate::export::{render_cdg, Palette};
        use crate::lyrics::{LyricLine, TimedLine};
        let render = |text: &str, overrides: Vec<Option<f64>>| {
            let mut l = LyricLine::new(text);
            l.word_overrides = overrides;
            l.sing_end_override = Some(11.5);
            let tl = TimedLine::from_line(&l, 8.0, 12.0);
            CdgPlayback::new(render_cdg(&[tl], 15.0, &Palette::default(), None, None))
        };
        let mut marked = render("beau|ti|ful", vec![Some(8.0), Some(9.5), Some(10.5)]);
        let mut plain = render("beautiful", vec![Some(8.0)]);
        let frame = |pb: &mut CdgPlayback, t: f64| {
            pb.seek(t);
            pb.rgba()
        };
        // Fully sung, both look identical - the `|` marks are never drawn.
        assert!(
            frame(&mut marked, 12.5) == frame(&mut plain, 12.5),
            "a syllable mark was drawn"
        );
        // In between, "beautiful" is already fully lit but the syllabified
        // one has only lit "beau", then "beau"+"ti".
        let lit = |pb: &mut CdgPlayback, t: f64, full: &[u8]| {
            frame(pb, t)
                .chunks(4)
                .zip(full.chunks(4))
                .filter(|(a, b)| a == b)
                .count()
        };
        let full = frame(&mut plain, 12.5);
        let (a, b, c) = (
            lit(&mut marked, 9.3, &full),
            lit(&mut marked, 10.3, &full),
            lit(&mut marked, 11.3, &full),
        );
        assert!(a < b && b < c && c == full.len() / 4, "{a} {b} {c}");
        assert_eq!(lit(&mut plain, 9.3, &full), full.len() / 4);
    }

    /// Regression for lines landing ~0.65s late: each line must be fully
    /// drawn by its start time, and its first word lit (with the preview
    /// row drawn too) within 0.2s - after which nothing changes until the
    /// second word, a second later.
    #[test]
    fn lines_are_drawn_by_their_start_and_first_word_lights_promptly() {
        use crate::export::{render_cdg, Palette};
        use crate::lyrics::{LyricLine, TimedLine};
        let mk = |text: &str, s: f64, e: f64, ov: Vec<Option<f64>>| {
            let mut l = LyricLine::new(text);
            l.word_overrides = ov;
            l.sing_end_override = Some(e);
            TimedLine::from_line(&l, s, e)
        };
        let cases: Vec<(&str, Vec<TimedLine>, Option<&str>, f64)> = vec![
            (
                "after an intro countdown",
                vec![mk("beautiful day", 8.0, 11.0, vec![Some(8.0), Some(9.0)])],
                None,
                8.0,
            ),
            (
                "first line under the title card",
                vec![mk("beautiful day", 2.5, 5.0, vec![Some(2.5), Some(3.5)])],
                Some("My Song Title"),
                2.5,
            ),
            (
                "back-to-back lines",
                vec![
                    mk(
                        "first line here ok",
                        1.0,
                        4.0,
                        vec![Some(1.0), Some(2.0), Some(3.0), Some(3.7)],
                    ),
                    mk(
                        "second line now",
                        4.0,
                        7.0,
                        vec![Some(4.0), Some(5.0), Some(6.0)],
                    ),
                ],
                None,
                4.0,
            ),
        ];
        for (name, lines, title, start) in cases {
            let mut pb =
                CdgPlayback::new(render_cdg(&lines, 12.0, &Palette::default(), title, None));
            let mut frame = |t: f64| {
                pb.seek(t);
                pb.rgba()
            };
            let diff =
                |a: &[u8], b: &[u8]| a.chunks(4).zip(b.chunks(4)).filter(|(x, y)| x != y).count();
            let before = frame(start - 0.6);
            let at_start = frame(start - 0.001);
            let first_word_lit = frame(start + 0.2);
            let just_before_second = frame(start + 0.95);
            assert!(
                diff(&before, &at_start) > 300,
                "{name}: line not drawn by its start"
            );
            assert!(
                diff(&at_start, &first_word_lit) > 0,
                "{name}: first word didn't light"
            );
            assert_eq!(
                diff(&first_word_lit, &just_before_second),
                0,
                "{name}: screen still changing after the first word - drawing ran late"
            );
        }
    }

    #[test]
    fn cdg_outro_card_clears_the_lyrics_and_draws_its_text() {
        use crate::export::{render_cdg_with_settings, Outro, Palette};
        use crate::lyrics::{LyricLine, TimedLine, TimingSettings};
        let mut l = LyricLine::new("Last line of the song");
        l.sing_end_override = Some(4.0);
        let tl = TimedLine::from_line(&l, 1.0, 5.0);
        let render = |outro: Option<&Outro>| {
            CdgPlayback::new(render_cdg_with_settings(
                std::slice::from_ref(&tl),
                &[],
                12.0,
                &Palette::default(),
                Some("Song"),
                Some("Band"),
                &TimingSettings::default(),
                outro,
            ))
        };
        let outro = Outro {
            start: 5.5,
            end: 12.0,
            lines: vec!["Thanks!".into()],
            show_title: true,
        };
        let mut with = render(Some(&outro));
        let mut without = render(None);
        let frame = |pb: &mut CdgPlayback, t: f64| {
            pb.seek(t);
            pb.rgba()
        };
        // Identical until the card is due.
        assert!(frame(&mut with, 5.4) == frame(&mut without, 5.4));
        // Then a different screen: the sung line is gone, the card is drawn.
        let card = frame(&mut with, 7.0);
        assert!(card != frame(&mut without, 7.0));
        let bg = card[..4].to_vec();
        assert!(card.chunks(4).filter(|p| *p != bg.as_slice()).count() > 300);
    }

    #[test]
    fn border_preset_only_touches_the_border() {
        let mut w = CdgWriter::new();
        w.memory_preset(2);
        w.border_preset(5);
        let mut d = CdgDecoder::default();
        for p in w.into_bytes().as_chunks::<PACKET_SIZE>().0 {
            d.apply(p);
        }
        assert_eq!(d.pixel(0, 0), 5);
        assert_eq!(d.pixel(WIDTH - 1, HEIGHT - 1), 5);
        assert_eq!(d.pixel(WIDTH / 2, HEIGHT / 2), 2);
    }

    #[test]
    fn seeking_backwards_and_forwards_matches_a_straight_decode() {
        // Two tile writes far apart in time, with checkpoints in between.
        let mut w = CdgWriter::new();
        w.memory_preset(0);
        w.tile_block(0, 0, 0, 3, &[0x3F; 12]);
        w.pad_until(12.0);
        w.tile_block(0, 0, 0, 7, &[0x3F; 12]);
        w.pad_until(20.0);
        let bytes = w.into_bytes();
        let mut pb = CdgPlayback::new(bytes);
        let (x, y) = (SAFE_COL_OFFSET as usize * 6, SAFE_ROW_OFFSET as usize * 12);
        let at = |pb: &mut CdgPlayback, t: f64| {
            pb.seek(t);
            pb.decoder.pixel(x, y)
        };
        assert_eq!(at(&mut pb, 15.0), 7);
        assert_eq!(at(&mut pb, 5.0), 3); // rewind via checkpoint
        assert_eq!(at(&mut pb, 11.99), 3);
        assert_eq!(at(&mut pb, 12.01), 7);
        assert_eq!(at(&mut pb, 0.0), 0); // nothing applied yet at t=0
        assert_eq!(at(&mut pb, 999.0), 7); // past the end clamps
    }
}
