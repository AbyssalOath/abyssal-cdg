//! Removes vocals from the loaded audio via real ML source separation,
//! using the `audio-separator` CLI (https://github.com/nomadkaraoke/python-audio-separator)
//! and its UVR-MDX-NET-Inst_HQ_3 model, instead of the old ffmpeg
//! phase-cancellation trick (subtracting one stereo channel from the
//! other) - that only worked on mixes with a dead-centered vocal and
//! audibly dulled anything else centered along with it (bass, kick, ...).
//! This is genuine source separation, not a crude approximation.
//!
//! `audio-separator` is a *separate* Python package this app shells out to
//! (the same way it already shells out to `ffmpeg`) - it is not bundled,
//! so removing vocals requires the user to install it themselves:
//!   pip install audio-separator          (CPU)
//!   pip install "audio-separator[gpu]"    (faster, needs a compatible GPU)
//! The model file (~100+MB) downloads automatically on first use and is
//! cached by audio-separator itself (not by this app) for later runs.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The UVR-MDX-NET model used for every separation - chosen for strong
/// instrumental quality. Not user-configurable (yet) - see the module docs
/// for why this needs `audio-separator` installed separately rather than
/// being bundled with the app.
const MODEL_FILENAME: &str = "UVR-MDX-NET-Inst_HQ_3.onnx";

/// Fixed base filename we ask audio-separator to use for the instrumental
/// stem (via `--custom_output_names`), so we know exactly what to look for
/// afterward regardless of the input's own filename.
const INSTRUMENTAL_STEM_NAME: &str = "instrumental";

/// Confirms `audio-separator` is installed and callable, with a clear,
/// actionable error message if not (removing vocals needs it; nothing
/// else in the app does).
pub fn check_audio_separator_available() -> Result<()> {
    let result = Command::new("audio-separator")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match result {
        Ok(status) if status.success() => Ok(()),
        _ => bail!(
            "audio-separator isn't installed (or isn't on your PATH). Removing vocals needs \
             it to run the UVR-MDX-NET separation model - it's a separate Python package, \
             not bundled with this app:\n\
             - Install Python 3, then run: pip install audio-separator\n\
             - For faster separation with a compatible Nvidia GPU: \
             pip install \"audio-separator[gpu]\"\n\
             The ~100+MB model file downloads automatically the first time you use it."
        ),
    }
}

/// Runs `input_path` through audio-separator's UVR-MDX-NET-Inst_HQ_3 model
/// and copies the resulting instrumental stem to `output_path` (format
/// inferred from `output_path`'s extension - `.mp3` and `.wav` both work).
/// Fails if `audio-separator` isn't installed, or if it itself errors.
///
/// This can take anywhere from several seconds to a few minutes depending
/// on song length and whether GPU acceleration is available - unlike the
/// old ffmpeg filter there's no meaningful progress percentage to report
/// along the way, just an eventual pass/fail.
pub fn remove_vocals_to_file(input_path: &Path, output_path: &Path) -> Result<()> {
    check_audio_separator_available()?;

    let temp_dir = std::env::temp_dir().join(format!(
        "abyssal-cdg-separator-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&temp_dir)
        .with_context(|| format!("failed to create temp dir {}", temp_dir.display()))?;

    let result = run_separation(input_path, output_path, &temp_dir);
    let _ = std::fs::remove_dir_all(&temp_dir);
    result
}

fn run_separation(input_path: &Path, output_path: &Path, temp_dir: &Path) -> Result<()> {
    let ext = output_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("wav")
        .to_ascii_uppercase();

    let mut cmd = Command::new("audio-separator");
    cmd.arg(input_path)
        .arg("--model_filename")
        .arg(MODEL_FILENAME)
        .arg("--output_dir")
        .arg(temp_dir)
        .arg("--output_format")
        .arg(&ext)
        .arg("--single_stem")
        .arg("Instrumental")
        .arg("--custom_output_names")
        .arg(format!(r#"{{"Instrumental":"{INSTRUMENTAL_STEM_NAME}"}}"#));
    if ext == "MP3" {
        cmd.arg("--output_bitrate").arg("320k");
    }

    let output = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("failed to launch audio-separator")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .lines()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        bail!("audio-separator failed:\n{tail}");
    }

    let produced = find_output_file(temp_dir, &ext)?;
    std::fs::copy(&produced, output_path)
        .with_context(|| format!("failed to save result to {}", output_path.display()))?;
    Ok(())
}

/// Locates the instrumental stem audio-separator just wrote into
/// `temp_dir`. Tries the exact name we asked for via `--custom_output_names`
/// first; if audio-separator's actual naming convention doesn't match that
/// assumption, falls back to "whatever single file is in there" - with
/// `--single_stem=Instrumental` there should only ever be one.
fn find_output_file(temp_dir: &Path, ext: &str) -> Result<PathBuf> {
    let expected = temp_dir.join(format!(
        "{INSTRUMENTAL_STEM_NAME}.{}",
        ext.to_ascii_lowercase()
    ));
    if expected.exists() {
        return Ok(expected);
    }
    let entries: Vec<PathBuf> = std::fs::read_dir(temp_dir)
        .with_context(|| format!("failed to read {}", temp_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    match entries.as_slice() {
        [only] => Ok(only.clone()),
        [] => bail!("audio-separator finished but produced no output file"),
        _ => bail!(
            "audio-separator produced more than one output file, expected exactly one \
             instrumental stem: {entries:?}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_input_file_fails_cleanly_instead_of_panicking() {
        let missing_in = Path::new("/nonexistent/definitely-not-a-real-file.wav");
        let out = std::env::temp_dir().join("abyssal-cdg-vocals-test-output.wav");
        let result = remove_vocals_to_file(missing_in, &out);
        assert!(result.is_err());
    }

    #[test]
    fn find_output_file_prefers_the_expected_name() {
        let dir = std::env::temp_dir().join(format!(
            "abyssal-cdg-vocals-test-find-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("instrumental.wav"), b"fake").unwrap();
        std::fs::write(dir.join("something_else.wav"), b"fake").unwrap();
        let found = find_output_file(&dir, "WAV").unwrap();
        assert_eq!(found, dir.join("instrumental.wav"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_output_file_falls_back_to_the_only_file_present() {
        let dir = std::env::temp_dir().join(format!(
            "abyssal-cdg-vocals-test-fallback-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("some_other_name.wav"), b"fake").unwrap();
        let found = find_output_file(&dir, "WAV").unwrap();
        assert_eq!(found, dir.join("some_other_name.wav"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_output_file_errors_on_empty_or_ambiguous_dir() {
        let dir = std::env::temp_dir().join(format!(
            "abyssal-cdg-vocals-test-empty-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(find_output_file(&dir, "WAV").is_err());

        std::fs::write(dir.join("a.wav"), b"fake").unwrap();
        std::fs::write(dir.join("b.wav"), b"fake").unwrap();
        assert!(find_output_file(&dir, "WAV").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
