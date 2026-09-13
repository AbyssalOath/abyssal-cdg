//! Turns characters into CDG tile bitmaps (6 pixels wide, 12 tall).
//!
//! We source glyphs from the `font8x8` crate, which gives us an 8x8 bitmap
//! per character (8 rows, each a `u8` where bit N is column N, column 0 on
//! the left). We use the left-most 6 columns (glyphs in this font don't use
//! columns 6-7, they're padding) and re-pack each row's bits into CDG's tile
//! format, where bit 5 is the leftmost pixel and bit 0 is the rightmost.
//! The 8 glyph rows are centered vertically inside the 12-row tile.

use crate::cdg::{TilePixels, BLANK_TILE};
use font8x8::{UnicodeFonts, BASIC_FONTS};

const GLYPH_ROWS: usize = 8;
const TILE_HEIGHT: usize = 12;
const VERTICAL_PAD_TOP: usize = 2; // (12 - 8) / 2

/// Reverse the low 6 bits of a byte: bit0<->bit5, bit1<->bit4, bit2<->bit3.
fn reverse6(b: u8) -> u8 {
    let mut out = 0u8;
    for i in 0..6 {
        if b & (1 << i) != 0 {
            out |= 1 << (5 - i);
        }
    }
    out
}

/// Render a single character into a 6x12 CDG tile bitmap. Unsupported
/// characters render as blank (space-like).
pub fn glyph_tile(ch: char) -> TilePixels {
    let Some(rows8) = BASIC_FONTS.get(ch) else {
        return BLANK_TILE;
    };
    let mut tile = BLANK_TILE;
    for (row_idx, src_row) in rows8.iter().enumerate().take(GLYPH_ROWS) {
        let six_bits = src_row & 0x3F; // columns 0..5, bit0=leftmost
        let cdg_bits = reverse6(six_bits); // bit5=leftmost, matches CDG tile format
        let dest_row = VERTICAL_PAD_TOP + row_idx;
        if dest_row < TILE_HEIGHT {
            tile[dest_row] = cdg_bits;
        }
    }
    tile
}

/// Render a character at an integer up-scale (1 = the normal 6x12 tile;
/// 2 = a 12x24 character built from a 2x2 grid of tiles, etc), by pixel-
/// doubling the base glyph. This is how we make CDG text bigger/more
/// legible than the native 6px-wide font allows, within the format's fixed
/// 300x216 canvas - fewer, larger characters per line rather than more
/// small ones. Returns a `scale x scale` grid of tiles, row-major.
pub fn glyph_tile_scaled(ch: char, scale: u8) -> Vec<Vec<TilePixels>> {
    let scale = scale.max(1);
    if scale == 1 {
        return vec![vec![glyph_tile(ch)]];
    }
    let scale = scale as usize;
    let Some(rows8) = BASIC_FONTS.get(ch) else {
        return vec![vec![BLANK_TILE; scale]; scale];
    };

    // Pixel-double the base 6-wide x 8-tall glyph into a (6*scale) x (8*scale) grid.
    let glyph_w = 6 * scale;
    let glyph_h = 8 * scale;
    let mut glyph = vec![vec![false; glyph_w]; glyph_h];
    for (src_row, byte) in rows8.iter().enumerate().take(GLYPH_ROWS) {
        let six_bits = byte & 0x3F; // bit i = column i, bit0 = leftmost
        for col in 0..6 {
            if six_bits & (1 << col) != 0 {
                for dy in 0..scale {
                    for dx in 0..scale {
                        glyph[src_row * scale + dy][col * scale + dx] = true;
                    }
                }
            }
        }
    }

    // Center that glyph vertically within a (6*scale) x (12*scale) canvas
    // (matching the same proportions as the unscaled tile).
    let canvas_w = 6 * scale;
    let canvas_h = 12 * scale;
    let pad_top = canvas_h.saturating_sub(glyph_h) / 2;
    let mut canvas = vec![vec![false; canvas_w]; canvas_h];
    for y in 0..glyph_h {
        if pad_top + y >= canvas_h {
            break;
        }
        for x in 0..glyph_w {
            canvas[pad_top + y][x] = glyph[y][x];
        }
    }

    // Slice the canvas into a `scale x scale` grid of proper 6x12 CDG tiles.
    let mut result = vec![vec![BLANK_TILE; scale]; scale];
    for (tile_row, row_out) in result.iter_mut().enumerate() {
        for (tile_col, tile_out) in row_out.iter_mut().enumerate() {
            let mut tile = BLANK_TILE;
            for r in 0..TILE_HEIGHT {
                let cy = tile_row * TILE_HEIGHT + r;
                let mut bits = 0u8;
                for c in 0..6 {
                    let cx = tile_col * 6 + c;
                    if canvas[cy][cx] {
                        bits |= 1 << (5 - c); // CDG bit order: bit5=leftmost
                    }
                }
                tile[r] = bits;
            }
            *tile_out = tile;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_is_blank() {
        assert_eq!(glyph_tile(' '), BLANK_TILE);
    }

    #[test]
    fn a_is_not_blank() {
        assert_ne!(glyph_tile('A'), BLANK_TILE);
    }

    #[test]
    fn reverse6_is_involution() {
        for b in 0..64u8 {
            assert_eq!(reverse6(reverse6(b)), b);
        }
    }

    #[test]
    fn scaled_glyph_geometry_matches_scale() {
        let g1 = glyph_tile_scaled('A', 1);
        assert_eq!(g1.len(), 1);
        assert_eq!(g1[0].len(), 1);
        assert_eq!(g1[0][0], glyph_tile('A'));

        let g2 = glyph_tile_scaled('A', 2);
        assert_eq!(g2.len(), 2);
        assert_eq!(g2[0].len(), 2);
        // At least one tile in the 2x2 grid should have visible pixels.
        assert!(g2.iter().flatten().any(|t| *t != BLANK_TILE));
    }

    #[test]
    fn scaled_space_is_still_blank() {
        let g2 = glyph_tile_scaled(' ', 2);
        for row in &g2 {
            for tile in row {
                assert_eq!(*tile, BLANK_TILE);
            }
        }
    }
}
