//! Locates and loads the ONNX Runtime shared library this app's two ML
//! features (`vocals.rs`/`mdx.rs` for vocal separation, `align.rs`/`ctc.rs`
//! for forced alignment) both need, via `ort`'s `load-dynamic` feature -
//! deliberately *not* `ort`'s `download-binaries` feature, which links
//! against a copy fetched at build time.
//!
//! Why: ONNX Runtime's own prebuilt binaries don't cover macOS x86_64
//! (Intel) - dropped upstream between versions 1.23 and 1.25, confirmed
//! directly against the actual release asset listings (no
//! `onnxruntime-osx-x64-*` asset exists for 1.28.0, the version this app
//! is pinned to, while `-linux-x64-`, `-osx-arm64-`, and `-win-x64-` all
//! do). That target needs a from-source build instead (cross-compiled on
//! an `arm64` runner - see `.github/workflows/release.yml`), and rather
//! than special-casing just that one platform, every platform uses the
//! same runtime-loading code path here: locate a bundled shared library
//! next to the installed app (or, in a dev build, a per-user cache
//! directory - see `model_assets.rs`, the same mechanism the ML model
//! weights use) and load it explicitly via `ort::init_from`.
//!
//! This must run once, before the first [`ort::session::Session`] is
//! created anywhere in the app - see [`ensure_loaded`].

use crate::model_assets;
use anyhow::{Context, Result};
use std::sync::OnceLock;

/// The exact ONNX Runtime version this app's bundled/downloaded binaries
/// are - must stay in sync with the version `ort-sys`'s own (unused, since
/// `load-dynamic` skips it) `dist.tsv` targets for this `ort` release, so
/// the C API this app links against at runtime matches what `ort`'s Rust
/// bindings expect.
pub const ONNXRUNTIME_VERSION: &str = "1.28.0";

static INIT: OnceLock<Result<(), String>> = OnceLock::new();

/// Platform-specific shared library filename, using the same convention
/// every official ONNX Runtime release archive uses:
/// `libonnxruntime.so` (Linux), `libonnxruntime.dylib` (macOS),
/// `onnxruntime.dll` (Windows).
fn dylib_filename() -> String {
    format!(
        "{}onnxruntime{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    )
}

/// The official ONNX Runtime release archive a developer can pull this
/// platform's shared library out of by hand - `None` for macOS x86_64,
/// which has no official archive to point at (see the module docs). Only
/// used to build the manual-setup error message in [`load`]; unlike the
/// ML model weights, this isn't auto-downloaded (it comes packed inside a
/// multi-file `.tgz`/`.zip` archive rather than being one file on its
/// own, and this path is dev-build-only - a real packaged release always
/// has this bundled - so it's not worth the extra archive-extraction
/// dependencies just for that convenience).
fn reference_archive_url() -> Option<String> {
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("linux-x64"),
        ("macos", "aarch64") => Some("osx-arm64"),
        ("windows", "x86_64") => Some("win-x64"),
        _ => None,
    }?;
    let ext = if std::env::consts::OS == "windows" {
        "zip"
    } else {
        "tgz"
    };
    Some(format!(
        "https://github.com/microsoft/onnxruntime/releases/download/v{v}/onnxruntime-{platform}-{v}.{ext}",
        v = ONNXRUNTIME_VERSION
    ))
}

/// Ensures ONNX Runtime is loaded - idempotent and cheap to call
/// repeatedly; the actual load only happens once per process.
pub fn ensure_loaded() -> Result<()> {
    INIT.get_or_init(|| load().map_err(|e| e.to_string()))
        .clone()
        .map_err(|e| anyhow::anyhow!(e))
}

fn load() -> Result<()> {
    let filename = dylib_filename();
    for candidate in model_assets::bundled_resource_candidates(&format!("onnxruntime/{filename}")) {
        if candidate.is_file() {
            return commit(&candidate);
        }
    }

    let cache_dir = model_assets::user_cache_subdir("onnxruntime")
        .context("couldn't determine a user cache directory")?;
    let cached = cache_dir.join(&filename);
    if cached.is_file() {
        return commit(&cached);
    }

    // Not bundled (expected for `cargo run`, since packager resources are
    // only present in a packaged build) and nothing cached yet - this is
    // a one-time manual setup step for a dev build, not something worth
    // auto-downloading-and-extracting a multi-file archive for.
    let instructions = match reference_archive_url() {
        Some(url) => format!(
            "download {url}, extract it, and copy its lib/{filename} to \
             {}",
            cached.display()
        ),
        None => format!(
            "macOS x86_64 has no official ONNX Runtime release to download - build one from \
             source (see .github/workflows/release.yml's macOS job) and copy the resulting \
             {filename} to {}",
            cached.display()
        ),
    };
    anyhow::bail!(
        "ONNX Runtime isn't available - this app's vocal separation and auto-align features \
         need it. A packaged release always bundles it; this only happens in a `cargo run` dev \
         build that hasn't set it up yet. To fix: {instructions}"
    );
}

fn commit(path: &std::path::Path) -> Result<()> {
    ort::init_from(path)
        .map_err(|e| anyhow::anyhow!("failed to load ONNX Runtime from {}: {e}", path.display()))?
        .commit();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dylib_filename_matches_the_current_platforms_convention() {
        let name = dylib_filename();
        assert!(name.contains("onnxruntime"));
        #[cfg(target_os = "windows")]
        assert_eq!(name, "onnxruntime.dll");
        #[cfg(target_os = "macos")]
        assert_eq!(name, "libonnxruntime.dylib");
        #[cfg(target_os = "linux")]
        assert_eq!(name, "libonnxruntime.so");
    }

    #[test]
    fn reference_archive_url_is_none_only_where_expected() {
        // Can't easily vary std::env::consts per-test, but this documents
        // and locks in the one platform this function must return None
        // for - see the module docs on why. A real cross-target CI run
        // (this project builds for all 4 targets) is what actually
        // exercises the other three branches.
        if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            assert!(reference_archive_url().is_none());
        } else {
            assert!(reference_archive_url().is_some());
        }
    }
}
