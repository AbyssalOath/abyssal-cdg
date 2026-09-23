//! Locates bundled ML model files (ONNX weights) shipped alongside the
//! installed app - vocal separation (`mdx.rs`) today, forced alignment in
//! a later phase.
//!
//! A real installed build gets these from `cargo-packager`'s bundled
//! resources (see `Cargo.toml`'s `[package.metadata.packager] resources`
//! and `.github/workflows/release.yml`, which fetches the model files
//! before packaging) - end users never need network access to use these
//! features. A `cargo run`/dev build has no such bundle, so this also
//! falls back to a per-user cache directory, downloading into it on first
//! use if the model isn't already there - a developer convenience and
//! safety net, not the shipped app's normal path.

use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Every plausible location for bundled resources relative to the running
/// executable, checked in order. `cargo-packager` lays resources out
/// differently per target - NSIS/WiX place them next to the executable,
/// macOS's `.app` bundle puts them under `Contents/Resources`, and Linux
/// deb/AppImage place them under `usr/lib`. Trying all of them (rather
/// than hard-coding the one for whatever platform this was last verified
/// on) means this doesn't have to be perfectly right about a packaging
/// layout that can't be built-and-run from this dev machine - any one of
/// them matching is enough.
/// `cargo-packager`'s Linux `.deb`/AppImage layout installs the binary at
/// `usr/bin/<pkg>` and bundles resources at `usr/lib/<pkg>/<relative>` -
/// confirmed directly against a real CI packaging run's AppImage build log
/// (which also showed the AppImage stage reusing the `.deb` stage's own
/// `usr/` tree wholesale, so both installs share this exact layout). This
/// app's own crate name is that `<pkg>` path segment.
const LINUX_PKG_DIR: &str = env!("CARGO_PKG_NAME");

pub(crate) fn bundled_resource_candidates(relative: &str) -> Vec<PathBuf> {
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let Some(exe_dir) = exe.parent().map(Path::to_path_buf) else {
        return Vec::new();
    };

    let mut candidates = vec![
        exe_dir.join(relative),
        exe_dir.join("resources").join(relative),
        exe_dir.join("lib").join(relative),
    ];
    if let Some(usr) = exe_dir.parent() {
        // Linux .deb/AppImage: exe is at usr/bin/<pkg>, resources at
        // usr/lib/<pkg>/<relative> - a sibling of bin/, not a child of it,
        // and one directory level deeper than the generic "lib" candidate
        // above (missing the <pkg> segment is what silently broke this on
        // the first real Linux packaged release - see LINUX_PKG_DIR docs).
        candidates.push(usr.join("lib").join(LINUX_PKG_DIR).join(relative));
    }
    if let Some(macos_contents) = exe_dir.parent() {
        candidates.push(macos_contents.join("Resources").join(relative));
    }
    // AppImage mounts itself and points $APPDIR at the mount root at
    // runtime - kept as a fallback alongside the exe-relative candidate
    // above, in case AppImage's exec wrapper ever makes current_exe()
    // resolve somewhere other than the real mounted path.
    if let Ok(appdir) = std::env::var("APPDIR") {
        candidates.push(
            Path::new(&appdir)
                .join("usr/lib")
                .join(LINUX_PKG_DIR)
                .join(relative),
        );
    }
    candidates
}

pub(crate) fn user_cache_subdir(name: &str) -> Result<PathBuf> {
    let base = dirs::cache_dir().context("couldn't determine a user cache directory")?;
    Ok(base.join("abyssal-cdg").join(name))
}

/// Finds `filename` (a model file normally bundled under `assets/models/`
/// at packaging time - see `bundled_resource_candidates`), trying bundled
/// locations first, then a per-user cache - downloading it from
/// `download_url` into that cache if it's not anywhere yet.
pub fn resolve_model(filename: &str, download_url: &str) -> Result<PathBuf> {
    for candidate in bundled_resource_candidates(&format!("models/{filename}")) {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    let cache_dir = user_cache_subdir("models")?;
    let cached = cache_dir.join(filename);
    if cached.is_file() {
        return Ok(cached);
    }

    std::fs::create_dir_all(&cache_dir)
        .with_context(|| format!("failed to create {}", cache_dir.display()))?;
    download_to_file(download_url, &cached).with_context(|| {
        format!(
            "failed to download the {filename} model (needed the first time this feature runs \
             in a dev build - a packaged release bundles it instead)"
        )
    })?;
    Ok(cached)
}

pub(crate) fn download_to_file(url: &str, dest: &Path) -> Result<()> {
    let response = ureq::get(url)
        .call()
        .with_context(|| format!("failed to request {url}"))?;
    if response.status().as_u16() >= 400 {
        bail!("request to {url} failed with HTTP {}", response.status());
    }

    let tmp = dest.with_extension("part");
    {
        let mut file = std::fs::File::create(&tmp)
            .with_context(|| format!("failed to create {}", tmp.display()))?;
        let mut reader = response.into_body().into_reader();
        let mut buf = [0u8; 256 * 1024];
        loop {
            let n = reader
                .read(&mut buf)
                .context("failed reading the download response body")?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .with_context(|| format!("failed writing to {}", tmp.display()))?;
        }
    }
    std::fs::rename(&tmp, dest)
        .with_context(|| format!("failed to finalize {}", dest.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_resource_candidates_includes_the_exe_directory_itself() {
        let candidates = bundled_resource_candidates("models/foo.onnx");
        assert!(!candidates.is_empty());
        let exe_dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert!(candidates.contains(&exe_dir.join("models/foo.onnx")));
    }

    #[test]
    fn resolve_model_finds_a_file_already_sitting_in_the_user_cache_dir() {
        let cache_dir = user_cache_subdir("models").unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();
        let name = format!("abyssal-cdg-test-model-{}.onnx", std::process::id());
        let path = cache_dir.join(&name);
        std::fs::write(&path, b"fake model bytes").unwrap();

        let found = resolve_model(&name, "https://example.invalid/unused").unwrap();
        assert_eq!(found, path);

        let _ = std::fs::remove_file(&path);
    }
}
