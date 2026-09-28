//! A tiny persisted "has this one-time prompt already been shown" flag -
//! currently just the legal disclaimer (see `main.rs`'s `draw_disclaimer_modal`
//! and `DISCLAIMER.md`), shown once on first launch and never again once
//! acknowledged. Same "small JSON file in the OS's per-user data directory"
//! pattern as `recent.rs`'s own persisted state, kept as its own file/module
//! rather than folded into `RecentFiles` since it isn't a "recently used
//! files" concern at all - a corrupt or missing file here just means the
//! disclaimer shows again, never a hard failure.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const ONBOARDING_FILENAME: &str = "onboarding.json";

#[derive(Default, Clone, Serialize, Deserialize)]
struct OnboardingState {
    #[serde(default)]
    disclaimer_acknowledged: bool,
}

fn onboarding_path(app_id: &str) -> Option<PathBuf> {
    Some(eframe::storage_dir(app_id)?.join(ONBOARDING_FILENAME))
}

fn load(app_id: &str) -> OnboardingState {
    let Some(path) = onboarding_path(app_id) else {
        return OnboardingState::default();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return OnboardingState::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// True once the disclaimer has already been acknowledged on this machine -
/// `false` on a genuinely first run (or if the state file is missing/
/// corrupt, which is treated the same as first-run: showing the disclaimer
/// again is the safe default, not silently skipping it).
pub fn disclaimer_acknowledged(app_id: &str) -> bool {
    load(app_id).disclaimer_acknowledged
}

/// Records that the disclaimer has been acknowledged, so it won't show
/// again on future launches. Errors writing the flag back to disk are
/// swallowed, same as `recent.rs`'s own saves - worth doing best-effort,
/// not worth interrupting the user over; worst case, the disclaimer just
/// shows again next launch.
pub fn acknowledge_disclaimer(app_id: &str) {
    let Some(path) = onboarding_path(app_id) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let state = OnboardingState {
        disclaimer_acknowledged: true,
    };
    if let Ok(json) = serde_json::to_string_pretty(&state) {
        let _ = std::fs::write(&path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Own app id (distinct from the real app's) so this never touches the
    // actual first-run flag a real run of the app might have left behind -
    // same reasoning as `project.rs`'s own autosave tests.
    fn test_app_id(label: &str) -> String {
        format!("abyssal-cdg-onboarding-test-{label}-{}", std::process::id())
    }

    #[test]
    fn disclaimer_is_unacknowledged_before_anything_is_written() {
        let app_id = test_app_id("unwritten");
        assert!(!disclaimer_acknowledged(&app_id));
    }

    #[test]
    fn acknowledging_persists_across_a_fresh_load() {
        let app_id = test_app_id("roundtrip");
        assert!(!disclaimer_acknowledged(&app_id));
        acknowledge_disclaimer(&app_id);
        assert!(disclaimer_acknowledged(&app_id));
    }
}
