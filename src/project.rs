//! Save/load project files (`.abyzl`) - a single JSON document capturing
//! everything needed to resume work later: the loaded audio's path, the
//! pasted lyrics text, every line's timing/overrides, title/artist, colors,
//! and the chosen video resolution. Everything else in `KaraokeApp` (the
//! live audio player itself, timeline zoom/scroll, in-progress text-field
//! editing state, ...) is session-only and gets rebuilt fresh from this on
//! load - see `KaraokeApp::apply_project_file` in `main.rs`.
//!
//! Format is plain JSON via `serde_json` - not because it's the most
//! compact option, but because a project file is small (lyrics + a handful
//! of numbers per line), and JSON is trivial to hand-inspect or hand-patch
//! if something ever goes wrong with one, which matters more than a few
//! saved bytes for a "your work" file.

use crate::lyrics::LyricLine;
use crate::video::{Background, BackgroundFit, Resolution};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Extension (without the dot) for project files, chosen to be
/// unambiguously "an Abyssal CDG Creator project" rather than a generic
/// `.json`/`.proj`.
pub const FILE_EXTENSION: &str = "abyzl";

/// Bumped whenever the on-disk shape changes in a way that needs explicit
/// migration logic. Purely additive changes (a new optional field) don't
/// need a bump - `serde`'s default-on-missing-field behavior (via
/// `#[serde(default)]`, added if/when such a field shows up) handles those
/// without breaking old files.
pub const CURRENT_VERSION: u32 = 1;

/// A plain 8-bit RGB color, independent of `egui::Color32` - this crate
/// doesn't otherwise depend on `egui`'s `serde` feature, and pulling that in
/// just for this would be a heavier dependency for one struct's worth of
/// (de)serialization. `main.rs` converts to/from `egui::Color32` at the
/// boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Every color the UI lets you customize, saved at full 8-bit fidelity
/// (not down-converted to CDG's 4-bit-per-channel palette) so reopening a
/// project and re-exporting the video doesn't lose precision a `.cdg`-only
/// round trip would have discarded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectColors {
    pub background: RgbColor,
    /// Color for a line whose singer hasn't been manually set. `None` in
    /// every project saved before this had its own color (via
    /// `#[serde(default)]`) - callers should fall back to `male_unsung`/
    /// `male_highlight` in that case, matching how `Default` used to always
    /// render identically to `Male`.
    #[serde(default)]
    pub default_unsung: Option<RgbColor>,
    #[serde(default)]
    pub default_highlight: Option<RgbColor>,
    pub male_unsung: RgbColor,
    pub male_highlight: RgbColor,
    pub female_unsung: RgbColor,
    pub female_highlight: RgbColor,
    pub duet_unsung: RgbColor,
    pub duet_highlight: RgbColor,
    pub preview: RgbColor,
    pub title: RgbColor,
    pub artist: RgbColor,
    pub screaming_unsung: RgbColor,
    pub screaming_highlight: RgbColor,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectFile {
    pub version: u32,
    /// Path to the audio file this project was timed against, if any -
    /// loaded automatically on open when it still exists at that path.
    pub audio_path: Option<PathBuf>,
    pub lyrics_raw: String,
    pub lines: Vec<LyricLine>,
    pub title: String,
    pub artist: String,
    pub colors: ProjectColors,
    pub video_resolution: Resolution,
    /// An image/video shown behind the lyrics in the video export/preview
    /// instead of a flat color fill - absent (`None`) in every project
    /// saved before this existed, via `#[serde(default)]`.
    #[serde(default)]
    pub background: Option<Background>,
    /// How `background` is scaled to fill the frame - defaults to `Cover`
    /// (crop to fill) for every project saved before this existed.
    #[serde(default)]
    pub background_fit: BackgroundFit,
    /// Opacity (0.0-1.0) of the black scrim blended over `background` so
    /// lyric text stays legible on top of busy/bright footage. Meaningless
    /// (and unused) without a `background` set.
    #[serde(default)]
    pub background_dim: f32,
    /// System font family for lyric text (video export + live preview
    /// only - see `fonts.rs`) - `None` (the default for every project
    /// saved before this existed, via `#[serde(default)]`) means the
    /// bundled default font. If the named family isn't installed on
    /// whatever machine opens this project, the app falls back to the
    /// default and shows a notice rather than failing to load.
    #[serde(default)]
    pub lyric_font_family: Option<String>,
}

impl ProjectFile {
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self).context("failed to serialize project")?;
        std::fs::write(path, json)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    pub fn load_from_file(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("failed to parse {} as a project file", path.display()))
    }
}

/// Ensures `path` ends in `.abyzl` (case-insensitively) - some native file
/// dialogs don't reliably auto-append a custom extension the OS doesn't
/// already recognize, so this is a safety net for both save and load
/// call sites that build a default filename.
pub fn ensure_project_extension(path: PathBuf) -> PathBuf {
    let has_it = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(FILE_EXTENSION));
    if has_it {
        path
    } else {
        let mut os_string = path.into_os_string();
        os_string.push(".");
        os_string.push(FILE_EXTENSION);
        PathBuf::from(os_string)
    }
}

/// Crash-recovery autosave: a single fixed-name file (not a project the
/// user explicitly chose) kept in the OS's per-user data directory for
/// this app - the same directory `eframe`'s own window-position
/// persistence uses, found via `eframe::storage_dir` (already pulled in by
/// this project's `persistence` feature, so no extra dependency needed).
/// Using a real per-user data directory - rather than, say, the current
/// working directory - means it's found on the next launch no matter how
/// the app happens to get started, and never collides with a project file
/// the user saved themselves.
///
/// The recovery flow this supports: `main.rs` overwrites this file
/// periodically while there's something worth recovering, then deletes it
/// on a clean shutdown (`eframe::App::on_exit`). If it's still there the
/// *next* time the app starts, that means the last run didn't get to clean
/// up - a crash, a force-quit, a system shutdown - so `main.rs` offers to
/// recover it before doing anything else.
const AUTOSAVE_FILENAME: &str = "autosave.abyzl";

/// Where the autosave lives for `app_id` (the same id passed to
/// `eframe::run_native`), or `None` if no per-user data directory could be
/// determined for this environment at all - autosave is simply skipped in
/// that case rather than guessing at a fallback that might not be writable
/// or might collide with something unrelated.
pub fn autosave_path(app_id: &str) -> Option<PathBuf> {
    Some(eframe::storage_dir(app_id)?.join(AUTOSAVE_FILENAME))
}

/// Overwrites the autosave slot with `project`'s current state. Creates
/// the data directory if it doesn't exist yet (e.g. first run).
pub fn write_autosave(app_id: &str, project: &ProjectFile) -> Result<()> {
    let path = autosave_path(app_id).context("no data directory available for autosave")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    project.save_to_file(&path)
}

/// Reads back a previous autosave, if the slot exists and parses cleanly.
/// Deliberately swallows errors into `None` rather than surfacing them - a
/// missing or corrupt recovery file just means "nothing to recover", not a
/// user-facing failure; the next periodic autosave will overwrite it
/// either way.
pub fn read_autosave(app_id: &str) -> Option<ProjectFile> {
    let path = autosave_path(app_id)?;
    if !path.exists() {
        return None;
    }
    ProjectFile::load_from_file(&path).ok()
}

/// Clears the autosave slot - called once the user has decided (recover or
/// discard) on a found autosave, and on every clean shutdown, so a stale
/// recovery prompt never lingers past the situation it was for.
pub fn clear_autosave(app_id: &str) {
    if let Some(path) = autosave_path(app_id) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::Singer;

    fn sample_colors() -> ProjectColors {
        let c = |n: u8| RgbColor {
            r: n,
            g: n.wrapping_add(1),
            b: n.wrapping_add(2),
        };
        ProjectColors {
            background: c(1),
            default_unsung: Some(c(13)),
            default_highlight: Some(c(14)),
            male_unsung: c(2),
            male_highlight: c(3),
            female_unsung: c(4),
            female_highlight: c(5),
            duet_unsung: c(6),
            duet_highlight: c(7),
            preview: c(8),
            title: c(9),
            artist: c(10),
            screaming_unsung: c(11),
            screaming_highlight: c(12),
        }
    }

    #[test]
    fn round_trips_through_a_real_file() {
        let mut lines = vec![LyricLine::new("hello world"), LyricLine::new("second line")];
        lines[0].start = Some(1.5);
        lines[0].singer = Singer::Duet;
        lines[0].sing_end_override = Some(3.25);
        lines[0].word_overrides[0] = Some(1.6);
        lines[0].word_end_overrides[1] = Some(3.0);
        let mut bv = crate::lyrics::BackingVocal::new("echo");
        bv.start = Some(2.0);
        bv.end = Some(2.8);
        bv.singer = Singer::Female;
        lines[0].backing_vocal = Some(bv);
        lines[1].start = Some(4.0);

        let project = ProjectFile {
            version: CURRENT_VERSION,
            audio_path: Some(PathBuf::from("/tmp/song.mp3")),
            lyrics_raw: "hello world\nsecond line".to_string(),
            lines,
            title: "Test Song".to_string(),
            artist: "Test Artist".to_string(),
            colors: sample_colors(),
            video_resolution: Resolution::Uhd4k,
            background: Some(Background::Image(PathBuf::from("/tmp/cover.png"))),
            background_fit: BackgroundFit::Contain,
            background_dim: 0.4,
            lyric_font_family: Some("Comic Sans MS".to_string()),
        };

        let dir = std::env::temp_dir().join(format!(
            "abyssal-cdg-project-test-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.abyzl");

        project.save_to_file(&path).unwrap();
        let loaded = ProjectFile::load_from_file(&path).unwrap();
        assert_eq!(loaded, project);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loads_a_pre_background_project_file_with_defaults() {
        // A project file saved before `background`/`background_dim`
        // existed - must still load, defaulting to no background.
        let dir = std::env::temp_dir().join(format!(
            "abyssal-cdg-project-test-old-format-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.abyzl");
        let json = serde_json::json!({
            "version": CURRENT_VERSION,
            "audio_path": null,
            "lyrics_raw": "hi",
            "lines": [],
            "title": "",
            "artist": "",
            "colors": sample_colors(),
            "video_resolution": "Hd1080",
        });
        std::fs::write(&path, serde_json::to_string(&json).unwrap()).unwrap();

        let loaded = ProjectFile::load_from_file(&path).unwrap();
        assert_eq!(loaded.background, None);
        assert_eq!(loaded.background_fit, BackgroundFit::Cover);
        assert_eq!(loaded.background_dim, 0.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_colors_without_a_saved_default_voice_color_deserializes_to_none() {
        // A project saved before `Default` had its own color - `colors`
        // omits `default_unsung`/`default_highlight` entirely. Callers
        // (see `KaraokeApp::apply_project_file`) fall back to `male_unsung`/
        // `male_highlight` in that case, matching how `Default` used to
        // always render identically to `Male`.
        let json = serde_json::json!({
            "background": {"r": 1, "g": 2, "b": 3},
            "male_unsung": {"r": 4, "g": 5, "b": 6},
            "male_highlight": {"r": 7, "g": 8, "b": 9},
            "female_unsung": {"r": 10, "g": 11, "b": 12},
            "female_highlight": {"r": 13, "g": 14, "b": 15},
            "duet_unsung": {"r": 16, "g": 17, "b": 18},
            "duet_highlight": {"r": 19, "g": 20, "b": 21},
            "preview": {"r": 22, "g": 23, "b": 24},
            "title": {"r": 25, "g": 26, "b": 27},
            "artist": {"r": 28, "g": 29, "b": 30},
            "screaming_unsung": {"r": 31, "g": 32, "b": 33},
            "screaming_highlight": {"r": 34, "g": 35, "b": 36},
        });
        let colors: ProjectColors = serde_json::from_value(json).unwrap();
        assert_eq!(colors.default_unsung, None);
        assert_eq!(colors.default_highlight, None);
    }

    #[test]
    fn load_from_file_fails_cleanly_on_a_missing_file() {
        let missing = Path::new("/nonexistent/definitely-not-a-real-project.abyzl");
        assert!(ProjectFile::load_from_file(missing).is_err());
    }

    #[test]
    fn load_from_file_fails_cleanly_on_garbage_content() {
        let dir = std::env::temp_dir().join(format!(
            "abyssal-cdg-project-test-garbage-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("garbage.abyzl");
        std::fs::write(&path, b"this is not json").unwrap();
        assert!(ProjectFile::load_from_file(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_project_extension_appends_when_missing_and_leaves_it_alone_otherwise() {
        assert_eq!(
            ensure_project_extension(PathBuf::from("song")),
            PathBuf::from("song.abyzl")
        );
        assert_eq!(
            ensure_project_extension(PathBuf::from("song.abyzl")),
            PathBuf::from("song.abyzl")
        );
        assert_eq!(
            ensure_project_extension(PathBuf::from("song.ABYZL")),
            PathBuf::from("song.ABYZL")
        );
        assert_eq!(
            ensure_project_extension(PathBuf::from("song.json")),
            PathBuf::from("song.json.abyzl")
        );
    }

    // Autosave tests use their own app id (distinct from the real app's) so
    // they never touch the actual crash-recovery slot a real run of the app
    // might have left behind.
    fn test_app_id(label: &str) -> String {
        format!("abyssal-cdg-test-{label}-{}", std::process::id())
    }

    #[test]
    fn autosave_round_trips_and_clears() {
        let app_id = test_app_id("roundtrip");
        // Sanity: nothing there yet.
        assert!(read_autosave(&app_id).is_none());

        let mut lines = vec![LyricLine::new("autosave me")];
        lines[0].start = Some(2.0);
        let project = ProjectFile {
            version: CURRENT_VERSION,
            audio_path: None,
            lyrics_raw: "autosave me".to_string(),
            lines,
            title: String::new(),
            artist: String::new(),
            colors: sample_colors(),
            video_resolution: Resolution::Hd1080,
            background: None,
            background_fit: BackgroundFit::default(),
            background_dim: 0.0,
            lyric_font_family: None,
        };

        write_autosave(&app_id, &project).unwrap();
        let recovered = read_autosave(&app_id).expect("autosave should be readable back");
        assert_eq!(recovered, project);

        clear_autosave(&app_id);
        assert!(read_autosave(&app_id).is_none());

        // Clearing twice (nothing there the second time) must not panic.
        clear_autosave(&app_id);

        // Tidy up the data directory `write_autosave` created for this
        // test's throwaway app id (the file itself is already gone).
        if let Some(dir) = eframe::storage_dir(&app_id) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn read_autosave_is_none_when_nothing_was_ever_written() {
        let app_id = test_app_id("never-written");
        assert!(read_autosave(&app_id).is_none());
    }
}
