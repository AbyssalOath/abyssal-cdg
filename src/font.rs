//! Turns characters into CDG tile bitmaps (6 pixels wide, 12 tall).
//!
//! We source glyphs from the `font8x8` crate, which gives us an 8x8 bitmap
//! per character (8 rows, each a `u8` where bit N is column N, column 0 on
//! the left). Many of its glyphs are 7 pixels wide (M, W, m, w, B, D, 0,
//! ...), one more than a CDG tile has room for, so [`fit_to_six_columns`]
//! narrows each glyph to 6 columns (simply cutting off column 6 used to
//! leave those letters visibly clipped). Each row is then re-packed into
//! CDG's tile format, where bit 5 is the leftmost pixel and bit 0 the
//! rightmost.
//! The 8 glyph rows are centered vertically inside the 12-row tile.
//!
//! font8x8's basic set is plain ASCII only, which on its own leaves curly
//! quotes/dashes (as pasted from almost every lyrics site) and accented
//! letters as blank gaps in the `.cdg`. [`cdg_glyph_rows`] works through a
//! fallback chain instead - see its doc comment. The user's lyric text is
//! never modified: substitution only happens at draw time, one character
//! for one character, so char offsets/word spans/wipe timing are unchanged.

use crate::cdg::{TilePixels, BLANK_TILE};
use font8x8::{UnicodeFonts, BASIC_FONTS, LATIN_FONTS};
use unicode_normalization::UnicodeNormalization;

type GlyphRows = [u8; 8];

/// Hand-drawn 6-pixel-wide glyphs for common lyric characters font8x8 has
/// no glyph for, and which don't have a single-character ASCII stand-in
/// that reads right. Same row format as font8x8 (bit 0 = leftmost column).
fn custom_glyph(ch: char) -> Option<GlyphRows> {
    match ch {
        // Horizontal ellipsis: three dots on the baseline row.
        '\u{2026}' => Some([0, 0, 0, 0, 0, 0, 0b0001_0101, 0]),
        _ => None,
    }
}

/// A plain single character to draw in place of `ch` when the font in use
/// has no glyph for it: curly quotes/primes -> straight quotes, the
/// various dashes/hyphens -> `-`, ellipsis -> `.`, and accented or
/// otherwise decorated Latin letters -> their base letter (via Unicode
/// canonical decomposition, e.g. `ő` -> `o`). Returns `None` if there's no
/// sensible stand-in (CJK, emoji, ...). Shared by the `.cdg` font here and
/// the video renderer (for a lyric font missing these glyphs).
pub fn fallback_char(ch: char) -> Option<char> {
    let mapped = match ch {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' | '\u{02BC}'
        | '\u{2035}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' | '\u{2036}' => '"',
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
        | '\u{2212}' => '-',
        '\u{2026}' => '.',
        '\u{2022}' | '\u{2027}' => '\u{00B7}',
        '\u{2039}' => '<',
        '\u{203A}' => '>',
        '\u{2044}' => '/',
        // Narrow/thin/figure/ideographic spaces etc.
        c if c != ' ' && c.is_whitespace() => ' ',
        c => {
            let base = c.nfd().next()?;
            if base == c {
                return None;
            }
            base
        }
    };
    Some(mapped)
}

/// Narrows an 8-column font8x8 glyph to the 6 columns a CDG tile holds
/// (bits 0-5 of each row, bit 0 = leftmost; bits 6-7 cleared). Empty
/// columns at the right edge go first, then the left edge - so a glyph that
/// already fits in columns 0-5 is returned unchanged. For a glyph genuinely
/// wider than 6, the most similar pair of adjacent columns is merged (OR'd)
/// instead: font8x8 draws with 2-pixel-thick strokes, so there's almost
/// always an identical pair to merge with nothing lost (ties prefer the
/// pair nearest the middle, where a merge is least noticeable).
fn fit_to_six_columns(rows: GlyphRows) -> GlyphRows {
    let mut cols: Vec<u8> = (0..8)
        .map(|c| {
            rows.iter()
                .enumerate()
                .fold(0u8, |acc, (r, row)| acc | (((row >> c) & 1) << r))
        })
        .collect();
    while cols.len() > 6 && *cols.last().unwrap() == 0 {
        cols.pop();
    }
    while cols.len() > 6 && cols[0] == 0 {
        cols.remove(0);
    }
    while cols.len() > 6 {
        let mid = (cols.len() as f32 - 2.0) / 2.0;
        let best = (0..cols.len() - 1)
            .min_by(|&a, &b| {
                let diff = |i: usize| (cols[i] ^ cols[i + 1]).count_ones();
                diff(a)
                    .cmp(&diff(b))
                    .then((a as f32 - mid).abs().total_cmp(&(b as f32 - mid).abs()))
            })
            .unwrap();
        let merged = cols[best] | cols[best + 1];
        cols[best] = merged;
        cols.remove(best + 1);
    }
    let mut out = [0u8; 8];
    for (c, col) in cols.iter().enumerate() {
        for (r, row) in out.iter_mut().enumerate() {
            *row |= ((col >> r) & 1) << c;
        }
    }
    out
}

/// The 8x8 bitmap to draw for `ch` in the `.cdg`, trying in order: the
/// ASCII set, the Latin-1 supplement set (é, ñ, ü, ç, ¿, ...), hand-drawn
/// extras ([`custom_glyph`]), then [`fallback_char`]'s stand-in looked up
/// the same way. `None` means it genuinely can't be drawn and will come
/// out blank - see [`cdg_unsupported_chars`] for warning about that.
/// Always narrowed to 6 columns (see [`fit_to_six_columns`]).
pub fn cdg_glyph_rows(ch: char) -> Option<GlyphRows> {
    let direct = |c: char| {
        BASIC_FONTS
            .get(c)
            .or_else(|| LATIN_FONTS.get(c))
            .or_else(|| custom_glyph(c))
    };
    direct(ch)
        .or_else(|| fallback_char(ch).and_then(direct))
        .map(fit_to_six_columns)
}

/// Every distinct character in `text` that the `.cdg` export can't draw
/// (even via fallback), in first-appearance order - the export dialog
/// lists these so a blank gap in the exported file isn't a surprise.
pub fn cdg_unsupported_chars(text: &str) -> Vec<char> {
    let mut out = Vec::new();
    for ch in text.chars() {
        if !ch.is_control() && cdg_glyph_rows(ch).is_none() && !out.contains(&ch) {
            out.push(ch);
        }
    }
    out
}

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

/// Render a single character into a 6x12 CDG tile bitmap. Characters with
/// no glyph even after [`cdg_glyph_rows`]'s fallbacks render as blank.
pub fn glyph_tile(ch: char) -> TilePixels {
    let Some(rows8) = cdg_glyph_rows(ch) else {
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
    let Some(rows8) = cdg_glyph_rows(ch) else {
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
            for (r, row_bits) in tile.iter_mut().enumerate() {
                let cy = tile_row * TILE_HEIGHT + r;
                let mut bits = 0u8;
                for c in 0..6 {
                    let cx = tile_col * 6 + c;
                    if canvas[cy][cx] {
                        bits |= 1 << (5 - c); // CDG bit order: bit5=leftmost
                    }
                }
                *row_bits = bits;
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
    fn curly_punctuation_draws_as_straight_ascii() {
        assert_eq!(glyph_tile('\u{2019}'), glyph_tile('\''));
        assert_eq!(glyph_tile('\u{2018}'), glyph_tile('\''));
        assert_eq!(glyph_tile('\u{201C}'), glyph_tile('"'));
        assert_eq!(glyph_tile('\u{201D}'), glyph_tile('"'));
        assert_eq!(glyph_tile('\u{2014}'), glyph_tile('-'));
        assert_eq!(glyph_tile('\u{2013}'), glyph_tile('-'));
    }

    #[test]
    fn ellipsis_has_its_own_glyph() {
        let t = glyph_tile('\u{2026}');
        assert_ne!(t, BLANK_TILE);
        assert_ne!(t, glyph_tile('.'));
    }

    #[test]
    fn latin1_accents_draw_from_latin_set() {
        for ch in ['é', 'ñ', 'ü', 'ç', 'à', 'Ö', '¿', '¡', 'ß'] {
            assert_ne!(glyph_tile(ch), BLANK_TILE, "{ch} should draw");
            assert!(cdg_glyph_rows(ch).is_some());
        }
        // Accented letters must not just fall back to their base letter.
        assert_ne!(glyph_tile('é'), glyph_tile('e'));
    }

    #[test]
    fn latin_extended_falls_back_to_base_letter() {
        // Not in font8x8's Latin-1 set - decomposes to its base letter.
        assert_eq!(glyph_tile('ő'), glyph_tile('o'));
        assert_eq!(glyph_tile('Š'), glyph_tile('S'));
    }

    #[test]
    fn unsupported_chars_are_reported_once_in_order() {
        let text = "Don\u{2019}t stop \u{2014} caf\u{00E9} 愛してる 愛";
        assert_eq!(cdg_unsupported_chars(text), vec!['愛', 'し', 'て', 'る']);
        assert!(cdg_unsupported_chars("Don\u{2019}t \u{201C}stop\u{201D}").is_empty());
    }

    #[test]
    fn every_glyph_fits_the_six_pixel_cell() {
        // We draw columns 0-5 only; anything left in 6-7 would be clipped.
        for ch in (0x20u32..0x7F)
            .chain(0xA0..0x100)
            .filter_map(char::from_u32)
        {
            if let Some(rows) = cdg_glyph_rows(ch) {
                assert!(rows.iter().all(|r| r & 0xC0 == 0), "{ch} uses columns 6-7");
            }
        }
    }

    #[test]
    fn narrow_glyphs_are_unchanged_by_fitting() {
        // 'o' and 'i' already sit within columns 0-5.
        for ch in ['o', 'i', 'l', '.', '!'] {
            let raw = BASIC_FONTS.get(ch).unwrap();
            assert!(raw.iter().all(|r| r & 0xC0 == 0));
            assert_eq!(cdg_glyph_rows(ch).unwrap(), raw, "{ch}");
        }
    }

    #[test]
    fn wide_glyphs_keep_both_outer_strokes() {
        // M/W/N span columns 0-6; clipping used to cut off the right leg.
        for ch in ['M', 'W', 'N', 'm', 'w'] {
            let rows = cdg_glyph_rows(ch).unwrap();
            let any_col = |c: u8| rows.iter().any(|r| r & (1 << c) != 0);
            assert!(any_col(0) && any_col(5), "{ch} lost an outer stroke");
        }
        // Merging identical stroke columns keeps M's top row two separate
        // legs (columns 0 and 5 set, a gap between them).
        let m = cdg_glyph_rows('M').unwrap();
        assert_eq!(m[0] & 0b10_0001, 0b10_0001);
        assert_eq!(m[0] & 0b00_1100, 0);
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
