//! Low-level CD+Graphics (CDG) packet encoder.
//!
//! A .cdg file is a stream of 24-byte "packets". Standard karaoke playback
//! (MP3+G / any CDG player) advances exactly one packet per 1/300th of a
//! second, so timing in the file is controlled purely by *how many packets*
//! come before a given instruction - there's no separate timestamp field.
//!
//! Packet layout (24 bytes):
//!   byte 0      : command      (0x09 marks "this is a CD+G command")
//!   byte 1      : instruction  (what to do)
//!   bytes 2-3   : parity Q     (we leave as 0; software players ignore it)
//!   bytes 4-19  : data         (16 bytes, instruction-specific)
//!   bytes 20-23 : parity P     (we leave as 0)
//!
//! Screen geometry: the visible CDG canvas is divided into 6x12-pixel tiles,
//! 50 columns x 18 rows. We keep to a safe visible area of 48 columns x 16
//! rows to avoid the overscan border that some players/TVs crop.

pub const PACKET_SIZE: usize = 24;
pub const PACKETS_PER_SEC: f64 = 300.0;

#[allow(dead_code)]
pub const TILE_COLS: u8 = 50;
#[allow(dead_code)]
pub const TILE_ROWS: u8 = 18;
/// Safe visible columns/rows (avoids the outer border tiles some players crop).
pub const SAFE_COLS: u8 = 48;
#[allow(dead_code)]
pub const SAFE_ROWS: u8 = 16;
pub const SAFE_COL_OFFSET: u8 = 1;
pub const SAFE_ROW_OFFSET: u8 = 1;

const INST_MEMORY_PRESET: u8 = 1;
const INST_BORDER_PRESET: u8 = 2;
const INST_TILE_BLOCK: u8 = 6;
#[allow(dead_code)]
const INST_TILE_BLOCK_XOR: u8 = 20;
const INST_LOAD_CLUT_LOW: u8 = 30;
const INST_LOAD_CLUT_HIGH: u8 = 31;

/// A 12-bit RGB color (each channel 0-15), as used by CDG's color table.
#[derive(Clone, Copy, Debug)]
pub struct CdgColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl CdgColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        // Clamp into 4-bit range defensively.
        Self { r: r & 0x0F, g: g & 0x0F, b: b & 0x0F }
    }
}

/// A 6(w) x 12(h) pixel tile bitmap. Each entry is one row; only the low 6
/// bits are used (bit 5 = leftmost pixel .. bit 0 = rightmost pixel).
pub type TilePixels = [u8; 12];

pub const BLANK_TILE: TilePixels = [0u8; 12];

pub struct CdgWriter {
    packets: Vec<[u8; PACKET_SIZE]>,
}

impl CdgWriter {
    pub fn new() -> Self {
        Self { packets: Vec::new() }
    }

    #[allow(dead_code)]
    pub fn len_packets(&self) -> usize {
        self.packets.len()
    }

    /// Current playback time (seconds) represented by the packets written so far.
    pub fn current_time_secs(&self) -> f64 {
        self.packets.len() as f64 / PACKETS_PER_SEC
    }

    fn push_packet(&mut self, instruction: u8, data: [u8; 16]) {
        let mut p = [0u8; PACKET_SIZE];
        p[0] = 0x09; // CD+G command marker
        p[1] = instruction & 0x3F;
        p[4..20].copy_from_slice(&data);
        self.packets.push(p);
    }

    /// A packet that does nothing - used as filler to keep timing in sync
    /// with the audio track between visible updates.
    pub fn push_filler(&mut self) {
        self.packets.push([0u8; PACKET_SIZE]);
    }

    /// Fill with no-op packets until the stream reaches `target_secs`.
    pub fn pad_until(&mut self, target_secs: f64) {
        let target_packets = (target_secs * PACKETS_PER_SEC).round() as usize;
        while self.packets.len() < target_packets {
            self.push_filler();
        }
    }

    /// Insert filler packets until we reach `target_secs`, but never move
    /// backwards - if we're already past that time, do nothing (this keeps
    /// closely-timed lyric events from corrupting the stream).
    pub fn advance_to(&mut self, target_secs: f64) {
        self.pad_until(target_secs);
    }

    pub fn memory_preset(&mut self, color_index: u8) {
        let mut data = [0u8; 16];
        data[0] = color_index & 0x0F;
        // repeat field (data[1]) left at 0; real players ignore preset repeats
        // fine for our purposes since we only send it once per clear.
        self.push_packet(INST_MEMORY_PRESET, data);
    }

    pub fn border_preset(&mut self, color_index: u8) {
        let mut data = [0u8; 16];
        data[0] = color_index & 0x0F;
        self.push_packet(INST_BORDER_PRESET, data);
    }

    /// Load 8 colors into the palette. `high` selects whether these fill
    /// palette indices 0-7 (false) or 8-15 (true).
    pub fn load_color_table(&mut self, colors: &[CdgColor; 8], high: bool) {
        let mut data = [0u8; 16];
        for (i, c) in colors.iter().enumerate() {
            // byte 1: 00 rrrr gg   (top 2 bits zero, then 4 bits red, top 2 bits of green)
            let b1 = ((c.r & 0x0F) << 2) | ((c.g & 0x0C) >> 2);
            // byte 2: 00 gg bbbb   (top 2 bits zero, bottom 2 bits of green, then 4 bits blue)
            let b2 = ((c.g & 0x03) << 4) | (c.b & 0x0F);
            data[i * 2] = b1 & 0x3F;
            data[i * 2 + 1] = b2 & 0x3F;
        }
        self.push_packet(if high { INST_LOAD_CLUT_HIGH } else { INST_LOAD_CLUT_LOW }, data);
    }

    /// Draw a tile at (row, col) in the *safe* coordinate space (0-based,
    /// 0..SAFE_ROWS / 0..SAFE_COLS) using color0 for unset bits and color1
    /// for set bits.
    pub fn tile_block(&mut self, row: u8, col: u8, color0: u8, color1: u8, pixels: &TilePixels) {
        let real_row = row + SAFE_ROW_OFFSET;
        let real_col = col + SAFE_COL_OFFSET;
        let mut data = [0u8; 16];
        data[0] = color0 & 0x0F;
        data[1] = color1 & 0x0F;
        data[2] = real_row & 0x1F;
        data[3] = real_col & 0x3F;
        for i in 0..12 {
            data[4 + i] = pixels[i] & 0x3F;
        }
        self.push_packet(INST_TILE_BLOCK, data);
    }

    pub fn into_bytes(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.packets.len() * PACKET_SIZE);
        for p in &self.packets {
            out.extend_from_slice(p);
        }
        out
    }
}

impl Default for CdgWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_is_24_bytes() {
        let mut w = CdgWriter::new();
        w.memory_preset(0);
        let bytes = w.into_bytes();
        assert_eq!(bytes.len(), 24);
        assert_eq!(bytes[0], 0x09);
        assert_eq!(bytes[1], 1);
    }

    #[test]
    fn timing_math() {
        let mut w = CdgWriter::new();
        w.pad_until(1.0);
        assert_eq!(w.len_packets(), 300);
        assert!((w.current_time_secs() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn advance_never_goes_backwards() {
        let mut w = CdgWriter::new();
        w.pad_until(2.0);
        let n = w.len_packets();
        w.advance_to(1.0); // earlier than current position
        assert_eq!(w.len_packets(), n); // no change
    }

    #[test]
    fn color_table_packs_12bit_rgb() {
        let mut w = CdgWriter::new();
        let colors = [CdgColor::new(15, 0, 0); 8];
        w.load_color_table(&colors, false);
        let bytes = w.into_bytes();
        assert_eq!(bytes[1], 30); // low CLUT instruction
        // byte1 = (r<<2)|(g>>2) = (15<<2)|0 = 60 = 0x3C
        assert_eq!(bytes[4] & 0x3F, 0x3C);
    }
}
