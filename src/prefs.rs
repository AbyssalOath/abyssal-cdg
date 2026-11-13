//! Per-machine user preferences that belong to the person and their setup
//! rather than to any one song - currently just the tap latency
//! compensation (see `main.rs`'s `compensate_tap`), the "Tips" toggle and
//! the last export folder and the UI size. Same "small JSON file in
//! the OS's per-user data directory" pattern as `onboarding.rs`/`recent.rs`:
//! a missing or corrupt file just means defaults, never a hard failure.
//!
//! Tap latency deliberately isn't saved with the project (`project.rs`):
//! it measures the user's reaction time plus their audio output delay
//! (Bluetooth headphones can add 150ms+), which follows the person and
//! their hardware from song to song, not the song.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const PREFS_FILENAME: &str = "preferences.json";

/// Upper bound for the tap latency setting - well beyond any realistic
/// reaction time plus Bluetooth delay, mainly a guard against a typo.
pub const MAX_TAP_LATENCY_MS: f64 = 1000.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    /// Milliseconds subtracted from every live tap (tap-along, re-tap,
    /// backing-line taps, and word/line-end taps in the fine-tune panel)
    /// to cancel out how late a tap lands after what was actually heard.
    #[serde(default)]
    pub tap_latency_ms: f64,
    /// Whether help text shows in full or as one-line summaries with an ⓘ
    /// (the "Tips" toggle). On for a fresh install (and for a preferences
    /// file saved before this existed), so first-time users get the
    /// explanations; experienced users can turn them off once.
    #[serde(default = "default_true")]
    pub show_tips: bool,
    /// The export folder last chosen in the export dialog, so it's
    /// already filled in next time (ignored at startup if it no longer
    /// exists).
    #[serde(default)]
    pub export_folder: Option<PathBuf>,
    /// UI size (egui zoom factor, 1.0 = 100%) - set with the top panel's
    /// UI size buttons or Ctrl+= / Ctrl+- / Ctrl+0.
    #[serde(default = "default_zoom")]
    pub ui_zoom: f32,
}

fn default_zoom() -> f32 {
    1.0
}

/// UI size limits (egui's own keyboard zoom allows 0.2-5.0; anything
/// outside this range in a saved file is clamped back in on load).
pub const MIN_UI_ZOOM: f32 = 0.5;
pub const MAX_UI_ZOOM: f32 = 3.0;

fn default_true() -> bool {
    true
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            tap_latency_ms: 0.0,
            show_tips: true,
            export_folder: None,
            ui_zoom: 1.0,
        }
    }
}

fn prefs_path(app_id: &str) -> Option<PathBuf> {
    Some(eframe::storage_dir(app_id)?.join(PREFS_FILENAME))
}

pub fn load(app_id: &str) -> Prefs {
    let Some(path) = prefs_path(app_id) else {
        return Prefs::default();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Prefs::default();
    };
    let mut prefs: Prefs = serde_json::from_str(&text).unwrap_or_default();
    if !prefs.tap_latency_ms.is_finite() {
        prefs.tap_latency_ms = 0.0;
    }
    prefs.tap_latency_ms = prefs.tap_latency_ms.clamp(0.0, MAX_TAP_LATENCY_MS);
    prefs.ui_zoom = if prefs.ui_zoom.is_finite() {
        prefs.ui_zoom.clamp(MIN_UI_ZOOM, MAX_UI_ZOOM)
    } else {
        1.0
    };
    prefs
}

/// Best-effort save, errors swallowed - same reasoning as
/// `onboarding::acknowledge_disclaimer`.
pub fn save(app_id: &str, prefs: &Prefs) {
    let Some(path) = prefs_path(app_id) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(prefs) {
        let _ = std::fs::write(&path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app_id(label: &str) -> String {
        format!("abyssal-cdg-prefs-test-{label}-{}", std::process::id())
    }

    #[test]
    fn defaults_before_anything_is_written() {
        assert_eq!(load(&test_app_id("unwritten")), Prefs::default());
    }

    #[test]
    fn tap_latency_round_trips() {
        let app_id = test_app_id("roundtrip");
        save(
            &app_id,
            &Prefs {
                tap_latency_ms: 140.0,
                show_tips: false,
                export_folder: Some(PathBuf::from("/music/karaoke")),
                ui_zoom: 1.5,
            },
        );
        assert_eq!(load(&app_id).tap_latency_ms, 140.0);
        assert!(!load(&app_id).show_tips);
        assert_eq!(
            load(&app_id).export_folder,
            Some(PathBuf::from("/music/karaoke"))
        );
        assert_eq!(load(&app_id).ui_zoom, 1.5);
    }

    #[test]
    fn tips_default_on_for_files_saved_before_the_setting_existed() {
        let app_id = test_app_id("old-file");
        let path = prefs_path(&app_id).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"tap_latency_ms": 50.0}"#).unwrap();
        let p = load(&app_id);
        assert_eq!(p.tap_latency_ms, 50.0);
        assert!(p.show_tips);
        assert_eq!(p.export_folder, None);
        assert_eq!(p.ui_zoom, 1.0);
    }

    #[test]
    fn out_of_range_latency_is_clamped_on_load() {
        let app_id = test_app_id("clamp");
        save(
            &app_id,
            &Prefs {
                tap_latency_ms: 99_999.0,
                show_tips: true,
                export_folder: None,
                ui_zoom: 99.0,
            },
        );
        assert_eq!(load(&app_id).tap_latency_ms, MAX_TAP_LATENCY_MS);
        assert_eq!(load(&app_id).ui_zoom, MAX_UI_ZOOM);
    }
}
