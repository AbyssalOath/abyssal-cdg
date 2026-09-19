//! Automatic word-level timing via forced alignment - shells out to
//! `aeneas` (https://github.com/readbeyond/aeneas) to align a line's own
//! words against the audio inside that line's own already-tapped time
//! window, instead of requiring every word to be tapped by hand in the
//! "Fine-tune words" panel.
//!
//! `aeneas` is a *separate* Python package this app shells out to (the same
//! way it already shells out to `ffmpeg` and `audio-separator`) - it is not
//! bundled, so this requires the user to install it themselves:
//!   pip install aeneas
//! (aeneas itself needs eSpeak/eSpeak-ng and ffmpeg on the system - see
//! https://github.com/readbeyond/aeneas for OS-specific setup notes).
//!
//! Design: rather than aligning the whole song's lyrics against the whole
//! file in one call (harder to keep accurate over minutes of audio, and an
//! all-or-nothing failure), each already-timed *line* gets its own call,
//! restricted to that line's own `[start, end)` window (padded a little,
//! see [`WINDOW_PAD_SECS`]) via aeneas's `is_audio_file_head_length` /
//! `is_audio_file_process_length` config keys. This directly builds on the
//! line-level taps the user already did: they're what defines each
//! alignment call's search window, so a mistimed *line* would need
//! re-tapping regardless, but a correctly-timed line gets much more
//! reliable word-level results than a single whole-song pass would.
//!
//! Word-level granularity is achieved by giving aeneas a plain-text input
//! file with *one word per line* - aeneas treats each line of a
//! `is_text_type=plain` input as one fragment to align regardless of what
//! it contains, so "one word per line" naturally produces one fragment per
//! word, without needing aeneas's more complex multilevel (`mplain`) input
//! format.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// How much extra audio to analyze on either side of a line's own tapped
/// `[start, end)` window - tapping is rarely frame-accurate, so a small pad
/// keeps the very first/last word from being clipped if the actual singing
/// starts slightly earlier or trails slightly later than tapped. The
/// caller is responsible for clamping this so it never eats into a
/// neighboring line's own window.
pub const WINDOW_PAD_SECS: f64 = 0.4;

/// One word's aligned window, in the same absolute (whole-song) timeline as
/// everything else in the app.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WordAlignment {
    pub start: f64,
    pub end: f64,
}

fn aeneas_python() -> String {
    std::env::var("ABYSSAL_CDG_PYTHON")
        .unwrap_or_else(|_| "python3".to_string())
}

/// Confirms `aeneas` is importable for `python3`, with a clear, actionable
/// error message if not. Checked via `import aeneas` rather than looking
/// for a standalone executable, since aeneas is invoked as a Python module
/// (`python3 -m aeneas.tools.execute_task`), not its own binary.
pub fn check_aligner_available() -> Result<()> {
    let result = Command::new(aeneas_python())
        .args(["-c", "import aeneas"])
        .output();
    match result {
        Ok(output) if output.status.success() => Ok(()),
        _ => bail!(
            "aeneas isn't installed for python3. Auto-align needs it to do forced alignment - \
             it's a separate Python package, not bundled with this app:\n\
             - Install Python 3, eSpeak (or eSpeak NG) and ffmpeg\n\
             - Then run: pip install aeneas\n\
             See https://github.com/readbeyond/aeneas for OS-specific setup notes."
        ),
    }
}

/// Aligns `words` (in order, one aeneas fragment each) against
/// `audio_path`, restricting analysis to `[window_start, window_end)`
/// seconds of the audio - returns one [`WordAlignment`] per word, in
/// absolute (whole-song) time. Fails (as a whole - no partial results) if
/// aeneas isn't installed, the process errors, or its output doesn't parse
/// into exactly `words.len()` fragments.
pub fn align_line(
    audio_path: &Path,
    words: &[&str],
    window_start: f64,
    window_end: f64,
    language: &str,
) -> Result<Vec<WordAlignment>> {
    if words.is_empty() {
        return Ok(Vec::new());
    }
    check_aligner_available()?;

    let temp_dir = std::env::temp_dir().join(format!(
        "abyssal-cdg-align-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&temp_dir)
        .with_context(|| format!("failed to create {}", temp_dir.display()))?;

    let result = align_line_in_dir(
        &temp_dir,
        audio_path,
        words,
        window_start,
        window_end,
        language,
    );
    let _ = std::fs::remove_dir_all(&temp_dir);
    result
}

fn align_line_in_dir(
    temp_dir: &Path,
    audio_path: &Path,
    words: &[&str],
    window_start: f64,
    window_end: f64,
    language: &str,
) -> Result<Vec<WordAlignment>> {
    let text_path = temp_dir.join("words.txt");
    let output_path = temp_dir.join("align.json");
    std::fs::write(&text_path, words.join("\n"))
        .with_context(|| format!("failed to write {}", text_path.display()))?;

    let head = window_start.max(0.0);
    let duration = (window_end - window_start).max(0.1);
    let config = format!(
        "task_language={language}|is_text_type=plain|os_task_file_format=json|\
         is_audio_file_head_length={head:.3}|is_audio_file_process_length={duration:.3}"
    );

    let output = Command::new(aeneas_python())
        .args(["-m", "aeneas.tools.execute_task"])
        .arg(audio_path)
        .arg(&text_path)
        .arg(&config)
        .arg(&output_path)
        .arg("--presets-word")
        .output()
        .context("failed to run aeneas (python3 -m aeneas.tools.execute_task)")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let message = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else {
            stdout.trim().to_string()
        };
        bail!("aeneas failed: {message}");
    }

    let json_text = std::fs::read_to_string(&output_path)
        .with_context(|| format!("aeneas didn't produce {}", output_path.display()))?;
    parse_sync_map(&json_text, words.len(), head)
}

/// Parses aeneas's flat (non-multilevel) JSON sync map - a
/// `{"fragments": [{"begin": "0.000", "end": "1.234", ...}, ...]}` array,
/// one entry per input line (here, one per word, since the input text has
/// one word per line).
///
/// aeneas is documented to report fragment times relative to the
/// *original* audio file even when `is_audio_file_head_length` restricts
/// where it searches (the head/tail is masked out of the search, not
/// physically trimmed beforehand). If some future/alternate build ever
/// reported times relative to the trimmed window instead, every fragment
/// would come back implausibly earlier than `head` - which is otherwise
/// impossible, since the search window starts there - so that case is
/// detected and corrected rather than trusted blindly either way.
fn parse_sync_map(json_text: &str, expected_words: usize, head: f64) -> Result<Vec<WordAlignment>> {
    let value: serde_json::Value =
        serde_json::from_str(json_text).context("aeneas output wasn't valid JSON")?;
    let fragments = value
        .get("fragments")
        .and_then(|f| f.as_array())
        .context("aeneas output had no \"fragments\" array")?;
    if fragments.len() != expected_words {
        bail!(
            "aeneas returned {} fragment(s), expected {expected_words}",
            fragments.len()
        );
    }

    let parse_time = |f: &serde_json::Value, key: &str| -> Result<f64> {
        f.get(key)
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .with_context(|| format!("fragment missing a numeric \"{key}\""))
    };
    let raw: Vec<(f64, f64)> = fragments
        .iter()
        .map(|f| Ok((parse_time(f, "begin")?, parse_time(f, "end")?)))
        .collect::<Result<_>>()?;

    let max_end = raw.iter().map(|&(_, e)| e).fold(0.0_f64, f64::max);
    let offset = if head > 0.05 && max_end < head - 0.05 {
        head
    } else {
        0.0
    };

    Ok(raw
        .into_iter()
        .map(|(begin, end)| WordAlignment {
            start: begin + offset,
            end: end + offset,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sync_map_json(fragments: &[(&str, &str)]) -> String {
        let entries: Vec<String> = fragments
            .iter()
            .map(|(b, e)| format!(r#"{{"begin": "{b}", "end": "{e}"}}"#))
            .collect();
        format!(r#"{{"fragments": [{}]}}"#, entries.join(","))
    }

    #[test]
    fn parses_absolute_times_unchanged() {
        let json = sync_map_json(&[("10.000", "10.500"), ("10.500", "11.200")]);
        let result = parse_sync_map(&json, 2, 10.0).unwrap();
        assert_eq!(
            result[0],
            WordAlignment {
                start: 10.0,
                end: 10.5
            }
        );
        assert_eq!(
            result[1],
            WordAlignment {
                start: 10.5,
                end: 11.2
            }
        );
    }

    #[test]
    fn detects_and_corrects_window_relative_times() {
        // Every fragment lands *before* head (10.0), which is impossible
        // for genuinely absolute times - so this must be window-relative.
        let json = sync_map_json(&[("0.000", "0.500"), ("0.500", "1.200")]);
        let result = parse_sync_map(&json, 2, 10.0).unwrap();
        assert_eq!(
            result[0],
            WordAlignment {
                start: 10.0,
                end: 10.5
            }
        );
        assert_eq!(
            result[1],
            WordAlignment {
                start: 10.5,
                end: 11.2
            }
        );
    }

    #[test]
    fn does_not_shift_when_head_is_zero() {
        let json = sync_map_json(&[("0.000", "0.500")]);
        let result = parse_sync_map(&json, 1, 0.0).unwrap();
        assert_eq!(
            result[0],
            WordAlignment {
                start: 0.0,
                end: 0.5
            }
        );
    }

    #[test]
    fn fails_cleanly_on_a_fragment_count_mismatch() {
        let json = sync_map_json(&[("0.000", "0.500")]);
        assert!(parse_sync_map(&json, 2, 0.0).is_err());
    }

    #[test]
    fn fails_cleanly_on_garbage_json() {
        assert!(parse_sync_map("not json", 1, 0.0).is_err());
    }

    #[test]
    fn fails_cleanly_when_fragments_key_is_missing() {
        assert!(parse_sync_map(r#"{"other": []}"#, 0, 0.0).is_err());
    }

    #[test]
    fn align_line_with_no_words_returns_empty_without_running_anything() {
        // Doesn't require aeneas to be installed - `words.is_empty()`
        // short-circuits before any check or subprocess call.
        let result = align_line(Path::new("/nonexistent.mp3"), &[], 0.0, 1.0, "eng").unwrap();
        assert!(result.is_empty());
    }
}
