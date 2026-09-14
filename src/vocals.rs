//! Reduces vocals in the loaded audio via phase-cancellation, so a user
//! doesn't have to run a separate command-line tool and juggle two copies
//! of the project just to have an instrumental to sing along with while
//! timing lyrics against the original (vocal) track.
//!
//! The technique: subtract each stereo channel from the other
//! (`c0=c0-c1`, `c1=c1-c0`). Anything mixed identically (same phase, same
//! level) into both channels cancels out - on a conventional stereo mix
//! that's usually the lead vocal, since it's typically panned dead center.
//! This is the same crude trick behind most "vocal remover" scripts/apps,
//! *not* real source separation - it also cancels any other centered
//! element (bass, kick, a centered lead guitar, ...), does nothing for a
//! mono source, and does little for a mix where the vocal isn't centered
//! or is doubled/widened. It's cheap and needs nothing beyond ffmpeg
//! (already required for video export), so it's worth trying first; a
//! genuinely clean instrumental needs real ML source separation (e.g.
//! Demucs) instead.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// ffmpeg audio filter graph implementing the phase-cancellation trick -
/// see the module docs for what it does and does not work well on.
const VOCAL_REMOVAL_FILTER: &str = "pan=stereo|c0=c0-c1|c1=c1-c0";

/// Runs `input_path` through the phase-cancellation filter and writes the
/// result to `output_path` (format inferred by ffmpeg from its extension -
/// `.mp3` and `.wav` both work). Fails if ffmpeg isn't installed/on PATH,
/// or if ffmpeg itself errors (e.g. an unreadable/corrupt input file).
pub fn remove_vocals_to_file(input_path: &Path, output_path: &Path) -> Result<()> {
    crate::video::check_ffmpeg_available()?;

    let output = Command::new("ffmpeg")
        .arg("-y")
        .arg("-i")
        .arg(input_path)
        .args(["-af", VOCAL_REMOVAL_FILTER])
        .arg(output_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("failed to launch ffmpeg")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .lines()
            .rev()
            .take(15)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        bail!("ffmpeg failed:\n{tail}");
    }
    Ok(())
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
}
