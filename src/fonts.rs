//! System font enumeration and loading, via the `font-kit` crate, for the
//! optional custom lyric-text font.
//!
//! **Scope: the video export and the live preview only.** The `.cdg`
//! export's font (`font.rs`) is a fixed 6x12-pixel, 1-bit bitmap tile
//! format sourced from the `font8x8` crate's purpose-built bitmap glyphs -
//! not a scalable font renderer, and not something an arbitrary system
//! TrueType/OpenType font can reasonably be downsampled into (most fonts
//! aren't designed to still read as letters at that resolution once
//! thresholded to 1 bit). The video export (`video.rs`, via `ab_glyph`)
//! and the live preview (`main.rs`, via `egui`'s own font system) are both
//! real scalable-font renderers, so a user-chosen system font applies to
//! those two and leaves the `.cdg` export on its dedicated bitmap font.

use anyhow::{bail, Context, Result};
use font_kit::handle::Handle;
use font_kit::properties::{Style, Weight};
use font_kit::source::SystemSource;

/// Fonts this app ships itself, baked into the binary at compile time -
/// unlike every other selectable font, which is loaded from whatever's
/// actually installed on the running machine (see [`load_family_bytes`]).
/// Both under the SIL Open Font License 1.1 (see their own `OFL.txt`
/// under `assets/fonts/`) - genuinely permissive (commercial use,
/// redistribution, embedding all fine, same class of license as DejaVu's
/// own Bitstream Vera), unlike an earlier "Bloodlust" font considered for
/// this exact spot and dropped for being non-commercial-use-only. Merged
/// into [`list_family_names`]'s result so they show up in the manual font
/// picker like any other choice, not just reachable via a color preset -
/// "Nosifer" is also the "Abyssal" color preset's own default font (see
/// `main.rs`'s `apply_color_preset`).
const BUNDLED_FONTS: &[(&str, &[u8])] = &[
    (
        "Creepster",
        include_bytes!("../assets/fonts/creepster/Creepster-Regular.ttf"),
    ),
    (
        "Nosifer",
        include_bytes!("../assets/fonts/nosifer/Nosifer-Regular.ttf"),
    ),
];

/// Every distinct font family name installed on this machine, plus this
/// app's own bundled fonts (see [`BUNDLED_FONTS`]), sorted and deduplicated
/// (font-kit's own listing can repeat a family name once per style/weight
/// file on some platforms/backends). Can take a noticeable moment on a
/// machine with a lot of fonts - callers should run this on a background
/// thread rather than blocking a frame on it.
pub fn list_family_names() -> Result<Vec<String>> {
    let source = SystemSource::new();
    let mut names = source
        .all_families()
        .context("failed to enumerate system fonts")?;
    names.extend(BUNDLED_FONTS.iter().map(|(name, _)| name.to_string()));
    Ok(dedup_sorted(names))
}

fn dedup_sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names.dedup();
    names
}

/// Loads the raw font file bytes for one representative face of
/// `family_name` - whichever face scores closest to "normal" (upright,
/// weight 400), since font-kit doesn't guarantee enumeration order puts the
/// regular weight first and a family listing Bold/Light/Italic before
/// Regular would otherwise pick an unexpected face. Used for both the video
/// export (`ab_glyph`) and the live preview (`egui`), which both just need
/// raw TrueType/OpenType bytes - font-kit's own `Font` wrapper isn't needed
/// past this point.
pub fn load_family_bytes(family_name: &str) -> Result<Vec<u8>> {
    if let Some((_, bytes)) = BUNDLED_FONTS.iter().find(|(name, _)| *name == family_name) {
        return Ok(bytes.to_vec());
    }
    let source = SystemSource::new();
    let family = source
        .select_family_by_name(family_name)
        .with_context(|| format!("font family \"{family_name}\" not found on this system"))?;
    let handles = family.fonts();
    let Some(first) = handles.first() else {
        bail!("font family \"{family_name}\" has no usable font file");
    };

    let best = handles
        .iter()
        .min_by_key(|h| match h.load() {
            Ok(f) => {
                let p = f.properties();
                normal_face_distance(p.style, p.weight)
            }
            // Couldn't load this particular face to inspect it - deprioritize
            // rather than discard, so a family with one broken face among
            // several still resolves to one of the good ones.
            Err(_) => (1, i32::MAX),
        })
        .unwrap_or(first);

    handle_to_bytes(best)
}

/// How far `(style, weight)` is from "normal" (upright, weight 400) - lower
/// is closer. A pure function purely so the selection heuristic is testable
/// without touching the real filesystem/font store.
fn normal_face_distance(style: Style, weight: Weight) -> (i32, i32) {
    let style_penalty = i32::from(style != Style::Normal);
    let weight_penalty = (weight.0 - Weight::NORMAL.0).abs() as i32;
    (style_penalty, weight_penalty)
}

fn handle_to_bytes(handle: &Handle) -> Result<Vec<u8>> {
    match handle {
        Handle::Path { path, .. } => std::fs::read(path)
            .with_context(|| format!("failed to read font file {}", path.display())),
        Handle::Memory { bytes, .. } => Ok((**bytes).clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_sorted_removes_duplicates_and_orders_alphabetically() {
        let names = vec![
            "Verdana".to_string(),
            "Arial".to_string(),
            "Verdana".to_string(),
            "Consolas".to_string(),
        ];
        assert_eq!(
            dedup_sorted(names),
            vec![
                "Arial".to_string(),
                "Consolas".to_string(),
                "Verdana".to_string()
            ]
        );
    }

    #[test]
    fn dedup_sorted_handles_empty_and_single_entry_lists() {
        assert_eq!(dedup_sorted(vec![]), Vec::<String>::new());
        assert_eq!(
            dedup_sorted(vec!["Arial".to_string()]),
            vec!["Arial".to_string()]
        );
    }

    #[test]
    fn normal_face_distance_prefers_upright_normal_weight() {
        let normal = normal_face_distance(Style::Normal, Weight::NORMAL);
        let bold = normal_face_distance(Style::Normal, Weight::BOLD);
        let italic = normal_face_distance(Style::Italic, Weight::NORMAL);
        let bold_italic = normal_face_distance(Style::Italic, Weight::BOLD);

        assert!(normal < bold);
        assert!(normal < italic);
        assert!(normal < bold_italic);
        assert_eq!(normal, (0, 0));
    }

    #[test]
    fn normal_face_distance_prefers_a_closer_weight_over_a_farther_one() {
        let light = normal_face_distance(Style::Normal, Weight(300.0));
        let black = normal_face_distance(Style::Normal, Weight(900.0));
        let closer = normal_face_distance(Style::Normal, Weight(450.0));
        assert!(closer < light);
        assert!(closer < black);
    }
}
