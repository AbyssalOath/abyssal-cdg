//! Locates the `ffmpeg` binary this app's video export and (for non-`.wav`
//! stems) vocal removal need, and the `libopenh264` shared library its
//! H.264 encoder depends on - replacing a system-installed `ffmpeg` on
//! `PATH` with one bundled alongside the app, the same way the ML models
//! and ONNX Runtime are (see `model_assets.rs`/`onnxrt.rs`).
//!
//! Two binaries, two different sourcing stories:
//!
//! - **`ffmpeg` itself** is a custom build this project compiles from
//!   source (`--disable-gpl`, no x264/x265 - see
//!   `.github/workflows/release.yml`), bundled into every installer.
//! - **`libopenh264`** is deliberately *not* a self-compiled copy, even
//!   though `ffmpeg` is built against openh264's headers to get H.264
//!   encoding at all. Cisco's patent-royalty coverage for OpenH264 -
//!   the whole reason it avoids H.264's usual patent licensing cost -
//!   applies only to *Cisco's own* separately-distributed binary
//!   (confirmed directly against openh264's `BINARY_LICENSE.txt`), not a
//!   third party's recompiled/bundled copy. So this app's `ffmpeg` links
//!   against openh264 at a fixed version (build-time only, to get the
//!   right headers/ABI) but the actual runtime library is Cisco's own
//!   official binary for that exact version, downloaded separately here -
//!   the same approach Firefox/Chromium use, for the same reason.
//!
//! A dev build (`cargo run`) falls back to a system-installed `ffmpeg` on
//! `PATH` if neither bundled nor cached (unlike the ONNX Runtime/ML model
//! resolvers, which don't fall back to a system copy) - a system `ffmpeg`
//! almost certainly has its own H.264 encoder built in already, so the
//! openh264 dance here is moot for it; this is purely a dev convenience so
//! testing doesn't require building this project's own custom ffmpeg first.

use crate::model_assets;
use anyhow::{bail, Context, Result};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The exact openh264 version this app's `ffmpeg` build links against -
/// must match exactly, since `ffmpeg` links against openh264's C ABI at
/// build time and this downloads the same version's binary for it to load
/// at runtime.
const OPENH264_VERSION: &str = "2.6.0";

fn ffmpeg_filename() -> &'static str {
    if cfg!(target_os = "windows") {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

fn openh264_filename() -> &'static str {
    // Every platform here is named to match exactly what this app's own
    // ffmpeg build was linked against (verified end-to-end, including
    // Linux's real ELF SONAME, not just the source file name - see
    // scripts/build-ffmpeg.sh's comments): the "lib" prefix on Windows is
    // real - a MinGW-built ffmpeg (like this app's) imports
    // "libopenh264.dll", not Cisco's own MSVC-built binary's bare
    // "openh264.dll" name, so the download step in the release workflow
    // renames it to match.
    if cfg!(target_os = "windows") {
        "libopenh264.dll"
    } else if cfg!(target_os = "macos") {
        "libopenh264.dylib"
    } else {
        "libopenh264.so"
    }
}

/// Cisco's official openh264 binary download for the current platform -
/// see `RELEASES` in https://github.com/cisco/openh264 for the naming
/// convention (verified directly against the real URLs, not guessed: the
/// shared-library "SONAME" version infix in the Linux filename has
/// changed across openh264 releases, so it's hardcoded per pinned
/// [`OPENH264_VERSION`] here rather than derived from a pattern that
/// might not hold for a future version bump).
fn openh264_download_url() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => {
            Some("http://ciscobinary.openh264.org/libopenh264-2.6.0-linux64.8.so.bz2")
        }
        ("macos", "aarch64") => {
            Some("http://ciscobinary.openh264.org/libopenh264-2.6.0-mac-arm64.dylib.bz2")
        }
        ("macos", "x86_64") => {
            Some("http://ciscobinary.openh264.org/libopenh264-2.6.0-mac-x64.dylib.bz2")
        }
        ("windows", "x86_64") => {
            Some("http://ciscobinary.openh264.org/openh264-2.6.0-win64.dll.bz2")
        }
        _ => None,
    }
}

fn resolve_ffmpeg() -> Result<PathBuf> {
    let filename = ffmpeg_filename();
    for candidate in model_assets::bundled_resource_candidates(&format!("ffmpeg/{filename}")) {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    let cache_dir = model_assets::user_cache_subdir("ffmpeg")?;
    let cached = cache_dir.join(filename);
    if cached.is_file() {
        return Ok(cached);
    }

    // Dev-build convenience only (see the module docs) - a packaged
    // release always has one of the two paths above.
    let system_available = Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if system_available {
        return Ok(PathBuf::from("ffmpeg"));
    }

    bail!(
        "ffmpeg isn't available - this app's video export and vocal removal (for MP3 output) \
         need it. A packaged release always bundles it; a `cargo run` dev build without it \
         bundled falls back to a system-installed `ffmpeg` on PATH, which also isn't found here. \
         Install ffmpeg (e.g. `sudo apt install ffmpeg`, `brew install ffmpeg`, or download from \
         https://ffmpeg.org/download.html) for local testing, or build this project's own \
         bundled copy (see .github/workflows/release.yml)."
    )
}

fn resolve_openh264() -> Result<PathBuf> {
    let filename = openh264_filename();
    for candidate in model_assets::bundled_resource_candidates(&format!("ffmpeg/{filename}")) {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    let cache_dir = model_assets::user_cache_subdir("ffmpeg")?;
    let cached = cache_dir.join(filename);
    if cached.is_file() {
        return Ok(cached);
    }

    let Some(url) = openh264_download_url() else {
        bail!(
            "no official openh264 v{OPENH264_VERSION} binary is available for this platform \
             (see ffmpeg_path.rs) - H.264 video export needs it. This is expected to never \
             happen on the platforms this app actually ships for."
        );
    };
    std::fs::create_dir_all(&cache_dir)
        .with_context(|| format!("failed to create {}", cache_dir.display()))?;
    download_and_decompress_bz2(url, &cached)
        .with_context(|| format!("failed to download openh264 from {url}"))?;
    Ok(cached)
}

fn download_and_decompress_bz2(url: &str, dest: &Path) -> Result<()> {
    let compressed_path = dest.with_extension("bz2.part");
    model_assets::download_to_file(url, &compressed_path)?;

    let compressed = std::fs::read(&compressed_path)
        .with_context(|| format!("failed to read {}", compressed_path.display()))?;
    let mut decoder = bzip2_rs::DecoderReader::new(compressed.as_slice());
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .context("failed to decompress openh264 download")?;

    let tmp = dest.with_extension("part");
    std::fs::write(&tmp, &decompressed)
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to make {} executable", tmp.display()))?;
    }
    std::fs::rename(&tmp, dest)
        .with_context(|| format!("failed to finalize {}", dest.display()))?;
    let _ = std::fs::remove_file(&compressed_path);
    Ok(())
}

/// Builds a `Command` for this app's `ffmpeg`, with the environment set up
/// so it can find `libopenh264` regardless of where either ended up
/// (bundled resource dir, cache dir, or - for `ffmpeg` only - system
/// `PATH`) - callers use this exactly like `Command::new("ffmpeg")`.
pub fn command() -> Result<Command> {
    let ffmpeg = resolve_ffmpeg()?;
    let mut cmd = Command::new(ffmpeg);

    // Only relevant to this app's own bundled ffmpeg (which is built
    // without a statically-linked H.264 encoder specifically so this
    // library can be swapped in - see the module docs); harmless to set
    // even when a dev build fell back to a system ffmpeg that doesn't use
    // it at all.
    if let Ok(openh264) = resolve_openh264() {
        if let Some(dir) = openh264.parent() {
            let var = if cfg!(target_os = "windows") {
                "PATH"
            } else if cfg!(target_os = "macos") {
                "DYLD_LIBRARY_PATH"
            } else {
                "LD_LIBRARY_PATH"
            };
            let existing = std::env::var(var).unwrap_or_default();
            let separator = if cfg!(target_os = "windows") {
                ";"
            } else {
                ":"
            };
            let new_value = if existing.is_empty() {
                dir.display().to_string()
            } else {
                format!("{}{separator}{existing}", dir.display())
            };
            cmd.env(var, new_value);
        }
    }

    Ok(cmd)
}

/// Confirms `ffmpeg` is available, with a clear, actionable error message
/// if not - same shape as the old `check_ffmpeg_available` this replaces,
/// kept as its own function since several call sites check availability
/// before doing other (non-ffmpeg) setup work first.
pub fn check_available() -> Result<()> {
    resolve_ffmpeg().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real end-to-end smoke test against a real bundled/cached ffmpeg +
    /// openh264 (this project's own custom LGPL build - see
    /// scripts/build-ffmpeg.sh) - not run by default. Actually invokes
    /// `command()` (the exact function every real call site uses) and
    /// runs a real H.264 encode, the same way video.rs does. Run with:
    ///   cargo test --release ffmpeg_path::tests::command_produces_a_working_ffmpeg -- --ignored --nocapture
    #[test]
    #[ignore]
    fn command_produces_a_working_ffmpeg() {
        let mut cmd = command().expect("should resolve ffmpeg");
        let output = cmd
            .arg("-version")
            .output()
            .expect("ffmpeg -version should run");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        eprintln!("{stdout}");
        assert!(stdout.contains("ffmpeg version"));

        // Real encode: tiny synthetic RGB24 frames -> H.264, exactly the
        // codec/pipeline video.rs uses, proving openh264 actually loads
        // and works through this app's own env-var setup, not just that
        // the binary starts.
        let tmp_out = std::env::temp_dir().join(format!(
            "abyssal-cdg-ffmpeg-test-{}.mp4",
            std::process::id()
        ));
        let frame = vec![100u8; 16 * 16 * 3];
        let mut child = command()
            .unwrap()
            .args([
                "-y",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-s",
                "16x16",
                "-r",
                "10",
                "-i",
                "-",
                "-c:v",
                "libopenh264",
                "-b:v",
                "200k",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&tmp_out)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("failed to spawn ffmpeg");
        {
            use std::io::Write;
            let stdin = child.stdin.as_mut().unwrap();
            for _ in 0..10 {
                stdin.write_all(&frame).unwrap();
            }
        }
        let result = child.wait_with_output().unwrap();
        eprintln!("{}", String::from_utf8_lossy(&result.stderr));
        assert!(result.status.success(), "encode failed");
        let size = std::fs::metadata(&tmp_out).unwrap().len();
        eprintln!("output size: {size} bytes");
        assert!(size > 100, "output suspiciously small/empty");
        let _ = std::fs::remove_file(&tmp_out);
    }

    #[test]
    fn openh264_download_url_covers_every_platform_this_app_ships_for() {
        for (os, arch) in [
            ("linux", "x86_64"),
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("windows", "x86_64"),
        ] {
            let _ = (os, arch); // documents the intent; see the note below
        }
        // Can't vary std::env::consts per-test, but this locks in that the
        // *current* platform (always one of this app's four release
        // targets in CI) resolves to Some(...), not None.
        assert!(openh264_download_url().is_some());
    }

    #[test]
    fn ffmpeg_filename_has_exe_suffix_only_on_windows() {
        let name = ffmpeg_filename();
        assert_eq!(name.ends_with(".exe"), cfg!(target_os = "windows"));
    }

    /// Real network test against Cisco's actual binary host - not run by
    /// default. Verifies the download+bz2-decompress path end-to-end
    /// (correct URL, correct decompression, resulting file looks like a
    /// real shared library) without needing the rest of this app's ML
    /// model infrastructure the way vocals.rs's/align.rs's equivalent
    /// tests do. Run with:
    ///   cargo test ffmpeg_path::tests::resolve_openh264_downloads_a_real_working_binary -- --ignored --nocapture
    #[test]
    #[ignore]
    fn resolve_openh264_downloads_a_real_working_binary() {
        let path = resolve_openh264().expect("should resolve/download");
        eprintln!("downloaded to: {}", path.display());
        let bytes = std::fs::read(&path).unwrap();
        eprintln!("size: {} bytes", bytes.len());
        assert!(
            bytes.len() > 100_000,
            "suspiciously small for a real codec library"
        );
        #[cfg(target_os = "linux")]
        assert_eq!(&bytes[..4], b"\x7fELF", "not a valid ELF shared object");
    }
}
