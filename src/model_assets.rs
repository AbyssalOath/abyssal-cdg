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
use sha2::{Digest, Sha256};
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
///
/// A freshly downloaded file is checked against `expected_sha256` (lowercase
/// hex) before being handed back, and deleted (never left in the cache for a
/// future run to pick up) if it doesn't match - this is consumed as ONNX
/// inference data, not executed as code, but a compromised/MITM'd download
/// still means silently wrong output for whoever's audio gets run through
/// it. A file already found bundled or already sitting in the cache from an
/// earlier successful download is *not* re-hashed here - that would mean
/// re-hashing a >1GB alignment model on every single use, for a check that
/// only protects against tampering *during* the download itself.
pub fn resolve_model(filename: &str, download_url: &str, expected_sha256: &str) -> Result<PathBuf> {
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
    if let Err(e) = verify_sha256(&cached, expected_sha256) {
        let _ = std::fs::remove_file(&cached);
        return Err(e);
    }
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

/// SHA-256 of `reader`'s full contents, as lowercase hex - shared by
/// [`verify_sha256`] (checking a file already on disk) and this module's
/// own tests (computing the expected hash of an in-memory fixture).
fn sha256_hex(mut reader: impl Read) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 256 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .context("failed reading data for hash verification")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Checks that `path`'s contents match `expected_hex` (a lowercase or
/// uppercase SHA-256 hex digest) - see [`resolve_model`] and
/// `ffmpeg_path.rs`'s openh264 download for where this guards a network
/// download before it's trusted (loaded as an ONNX model, or `dlopen`'d as
/// native code).
pub(crate) fn verify_sha256(path: &Path, expected_hex: &str) -> Result<()> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open {} for hash verification", path.display()))?;
    let actual = sha256_hex(file)?;
    if !actual.eq_ignore_ascii_case(expected_hex) {
        bail!(
            "{} failed hash verification (expected {expected_hex}, got {actual}) - the download \
             may be corrupted or tampered with",
            path.display()
        );
    }
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

        // A file already sitting in the cache is never re-hashed (see
        // `resolve_model`'s docs), so an obviously-wrong hash here still
        // succeeds - this test is specifically about the cache-hit path,
        // not verification.
        let found = resolve_model(&name, "https://example.invalid/unused", "0000").unwrap();
        assert_eq!(found, path);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn verify_sha256_accepts_a_matching_hash_case_insensitively() {
        let path =
            std::env::temp_dir().join(format!("abyssal-cdg-test-hash-ok-{}", std::process::id()));
        std::fs::write(&path, b"hello world").unwrap();
        let expected = sha256_hex(b"hello world".as_slice()).unwrap();

        assert!(verify_sha256(&path, &expected).is_ok());
        assert!(verify_sha256(&path, &expected.to_uppercase()).is_ok());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn verify_sha256_rejects_a_mismatched_hash() {
        let path =
            std::env::temp_dir().join(format!("abyssal-cdg-test-hash-bad-{}", std::process::id()));
        std::fs::write(&path, b"hello world").unwrap();

        let err = verify_sha256(&path, &"0".repeat(64)).unwrap_err();
        assert!(
            err.to_string().contains("failed hash verification"),
            "unexpected error: {err}"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// A minimal single-request HTTP/1.1 server for exercising the real
    /// download path (`download_to_file`/`resolve_model`) without hitting
    /// the network - binds an ephemeral port, serves `body` for exactly one
    /// request, then exits.
    fn spawn_test_http_server(body: Vec<u8>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut discard = [0u8; 4096];
                let _ = stream.read(&mut discard);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn resolve_model_deletes_a_download_that_fails_hash_verification() {
        let url = spawn_test_http_server(b"not the real model".to_vec());
        let cache_dir = user_cache_subdir("models").unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();
        let name = format!("abyssal-cdg-test-badhash-{}.onnx", std::process::id());
        let cached_path = cache_dir.join(&name);
        let _ = std::fs::remove_file(&cached_path);

        let result = resolve_model(&name, &url, &"0".repeat(64));

        assert!(result.is_err());
        assert!(
            !cached_path.is_file(),
            "a download that fails verification must not be left in the cache"
        );
    }

    #[test]
    fn resolve_model_keeps_a_download_that_matches_its_pinned_hash() {
        let body = b"the real model".to_vec();
        let expected = sha256_hex(body.as_slice()).unwrap();
        let url = spawn_test_http_server(body.clone());
        let cache_dir = user_cache_subdir("models").unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();
        let name = format!("abyssal-cdg-test-goodhash-{}.onnx", std::process::id());
        let cached_path = cache_dir.join(&name);
        let _ = std::fs::remove_file(&cached_path);

        let found = resolve_model(&name, &url, &expected).unwrap();

        assert_eq!(found, cached_path);
        assert_eq!(std::fs::read(&cached_path).unwrap(), body);
        let _ = std::fs::remove_file(&cached_path);
    }
}
