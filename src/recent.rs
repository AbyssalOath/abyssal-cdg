//! A small persisted "recently used files" list, so re-opening something
//! you worked on yesterday doesn't mean hunting through a file picker
//! again. Separate lists for project files and audio files, each capped
//! and most-recent-first. Stored next to the crash-recovery autosave in
//! the OS's per-user data directory (see `project::autosave_path` for why
//! that location, and `eframe::storage_dir`).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAX_ENTRIES: usize = 8;
const RECENT_FILENAME: &str = "recent-files.json";

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct RecentFiles {
    pub projects: Vec<PathBuf>,
    pub audio: Vec<PathBuf>,
}

fn recent_path(app_id: &str) -> Option<PathBuf> {
    Some(eframe::storage_dir(app_id)?.join(RECENT_FILENAME))
}

/// Reads back the persisted list, or an empty one if there isn't one yet
/// (first run) or it fails to parse - a corrupt recent-files list is worth
/// silently starting over from, not surfacing as an error.
pub fn load(app_id: &str) -> RecentFiles {
    let Some(path) = recent_path(app_id) else {
        return RecentFiles::default();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return RecentFiles::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save(app_id: &str, recent: &RecentFiles) -> Result<()> {
    let path = recent_path(app_id).context("no data directory available")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(recent).context("failed to serialize recent files")?;
    std::fs::write(&path, json).with_context(|| format!("failed to write {}", path.display()))
}

/// Moves `path` to the front of `list`, removing a prior occurrence first
/// (so re-opening something already in the list doesn't duplicate it) and
/// trimming to [`MAX_ENTRIES`].
fn push_front(list: &mut Vec<PathBuf>, path: PathBuf) {
    list.retain(|p| p != &path);
    list.insert(0, path);
    list.truncate(MAX_ENTRIES);
}

/// Records that `path` was just opened/saved as a project, moving it to the
/// front of the recent-projects list. Errors writing the list back to disk
/// are swallowed - worth doing best-effort, not worth interrupting the
/// user's actual save/load over.
pub fn record_project(app_id: &str, path: &Path) {
    let mut recent = load(app_id);
    push_front(&mut recent.projects, path.to_path_buf());
    let _ = save(app_id, &recent);
}

/// Same as [`record_project`], for the recent-audio list.
pub fn record_audio(app_id: &str, path: &Path) {
    let mut recent = load(app_id);
    push_front(&mut recent.audio, path.to_path_buf());
    let _ = save(app_id, &recent);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app_id(label: &str) -> String {
        format!("abyssal-cdg-test-recent-{label}-{}", std::process::id())
    }

    fn cleanup(app_id: &str) {
        if let Some(dir) = eframe::storage_dir(app_id) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn push_front_dedupes_and_moves_to_front() {
        let mut list = vec![PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")];
        push_front(&mut list, PathBuf::from("b"));
        assert_eq!(
            list,
            vec![PathBuf::from("b"), PathBuf::from("a"), PathBuf::from("c")]
        );
    }

    #[test]
    fn push_front_trims_to_max_entries() {
        let mut list: Vec<PathBuf> = (0..MAX_ENTRIES)
            .map(|i| PathBuf::from(format!("file{i}")))
            .collect();
        push_front(&mut list, PathBuf::from("new"));
        assert_eq!(list.len(), MAX_ENTRIES);
        assert_eq!(list[0], PathBuf::from("new"));
    }

    #[test]
    fn record_project_round_trips_through_a_real_file() {
        let app_id = test_app_id("project-roundtrip");
        assert!(load(&app_id).projects.is_empty());

        record_project(&app_id, Path::new("/tmp/song.abyzl"));
        record_project(&app_id, Path::new("/tmp/other.abyzl"));

        let recent = load(&app_id);
        assert_eq!(
            recent.projects,
            vec![
                PathBuf::from("/tmp/other.abyzl"),
                PathBuf::from("/tmp/song.abyzl")
            ]
        );
        assert!(recent.audio.is_empty());

        cleanup(&app_id);
    }

    #[test]
    fn record_audio_is_independent_of_projects() {
        let app_id = test_app_id("audio-independent");
        record_project(&app_id, Path::new("/tmp/song.abyzl"));
        record_audio(&app_id, Path::new("/tmp/song.mp3"));

        let recent = load(&app_id);
        assert_eq!(recent.projects, vec![PathBuf::from("/tmp/song.abyzl")]);
        assert_eq!(recent.audio, vec![PathBuf::from("/tmp/song.mp3")]);

        cleanup(&app_id);
    }
}
