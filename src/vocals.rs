//! Removes vocals from the loaded audio via real ML source separation -
//! native Rust, running the actual UVR-MDX-NET-Inst_HQ_3 ONNX weights
//! through `ort` (ONNX Runtime) directly (see `mdx.rs` for the separation
//! algorithm and `stft.rs` for its STFT/ISTFT). This used to shell out to
//! the `audio-separator` Python CLI; now nothing needs to be installed
//! separately - the model ships with the app (see `model_assets.rs`) and
//! inference runs in-process.
//!
//! Produces *both* stems from a single separation pass - the vocals stem
//! is derived from the instrumental one by subtraction (see `mdx.rs`), not
//! a second, independently expensive model run, so exposing both here
//! costs nothing extra over the old instrumental-only feature.

use crate::mdx::MdxSeparator;
use crate::model_assets;
use anyhow::{Context, Result};
use rodio::{Decoder, Source};
use rubato::audioadapter::Adapter;
use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler, WindowFunction};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::Stdio;

const MODEL_FILENAME: &str = "UVR-MDX-NET-Inst_HQ_3.onnx";
const MODEL_DOWNLOAD_URL: &str = "https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models/UVR-MDX-NET-Inst_HQ_3.onnx";
/// UVR-MDX-NET-Inst_HQ_3 (like every MDX-Net model) is trained on fixed
/// 44.1kHz audio - input at any other rate is resampled to this before
/// separation (see `resample_to`).
const MODEL_SAMPLE_RATE: u32 = 44_100;

/// The result of one separation pass - both stems, same sample rate as
/// [`MODEL_SAMPLE_RATE`].
pub struct Separation {
    pub sample_rate: u32,
    /// Music with vocals removed.
    pub instrumental: [Vec<f32>; 2],
    /// Vocals only.
    pub vocals: [Vec<f32>; 2],
}

/// Locates (bundled, cached, or freshly downloaded - see `model_assets.rs`)
/// the separation model and loads it. Callers that only need one
/// separation can skip this and just call [`separate`]; this exists so a
/// caller doing several in a row (e.g. instrumental export *and* video
/// vocal removal in the same export run) can load the ~65MB model once
/// and reuse it, instead of paying that cost twice.
pub fn load_separator() -> Result<MdxSeparator> {
    let model_path = model_assets::resolve_model(MODEL_FILENAME, MODEL_DOWNLOAD_URL)
        .context("couldn't locate or download the vocal separation model")?;
    MdxSeparator::load(&model_path)
}

/// Separates `input_path` into instrumental and vocals, calling
/// `on_progress(0.0..=1.0)` as it goes.
pub fn separate(
    separator: &mut MdxSeparator,
    input_path: &Path,
    on_progress: impl FnMut(f32),
) -> Result<Separation> {
    let mix = load_stereo_44100(input_path)?;
    let result = separator
        .separate(&mix, on_progress)
        .context("vocal separation failed")?;
    Ok(Separation {
        sample_rate: MODEL_SAMPLE_RATE,
        instrumental: result.instrumental,
        vocals: result.vocals,
    })
}

/// Separates `input_path` and writes just the vocals stem out to a fresh
/// temp WAV file, returning its path - used by `align.rs`'s auto-align
/// (via `main.rs`'s `start_word_alignment`) to run forced alignment
/// against isolated vocals instead of the full mix, since the CTC speech
/// model aligns more reliably without instrumentation underneath the
/// singing. The caller owns the returned path and is responsible for
/// deleting it once alignment is done with it (this doesn't clean up
/// after itself, the same way `write_stem_to_file`'s own intermediate
/// `.wav.tmp` only cleans up the one it makes internally, not this one).
pub fn separate_vocals_to_temp_wav(
    input_path: &Path,
    on_progress: impl FnMut(f32),
) -> Result<PathBuf> {
    let mut separator = load_separator()?;
    let separation = separate(&mut separator, input_path, on_progress)?;
    let tmp_path = std::env::temp_dir().join(format!(
        "abyssal-cdg-align-vocals-{}.wav",
        std::process::id()
    ));
    write_stem_to_file(&separation.vocals, separation.sample_rate, &tmp_path)?;
    Ok(tmp_path)
}

/// Writes one stem to `output_path` (format inferred from its extension -
/// `.wav` is written directly, anything else is converted from an
/// intermediate WAV via `ffmpeg`, the same one the video exporter already
/// depends on).
pub fn write_stem_to_file(
    stem: &[Vec<f32>; 2],
    sample_rate: u32,
    output_path: &Path,
) -> Result<()> {
    let ext = output_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("wav")
        .to_ascii_lowercase();

    if ext == "wav" {
        write_wav(stem, sample_rate, output_path)
    } else {
        let tmp = output_path.with_extension("wav.tmp");
        write_wav(stem, sample_rate, &tmp)?;
        let result = encode_via_ffmpeg(&tmp, output_path, &ext);
        let _ = std::fs::remove_file(&tmp);
        result
    }
}

fn write_wav(stem: &[Vec<f32>; 2], sample_rate: u32, path: &Path) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("failed to create {}", path.display()))?;
    for (&l, &r) in stem[0].iter().zip(stem[1].iter()) {
        writer.write_sample(l)?;
        writer.write_sample(r)?;
    }
    writer
        .finalize()
        .with_context(|| format!("failed to finalize {}", path.display()))?;
    Ok(())
}

/// Converts a WAV to another format via `ffmpeg` - this app still shells
/// out to it for video export (see `video.rs`), so this doesn't add a new
/// runtime dependency, just reuses the one that's still there until that's
/// addressed separately.
fn encode_via_ffmpeg(wav_path: &Path, output_path: &Path, ext: &str) -> Result<()> {
    let mut cmd = crate::ffmpeg_path::command()?;
    cmd.args(["-y", "-i"]).arg(wav_path);
    if ext == "mp3" {
        cmd.args(["-b:a", "320k"]);
    }
    cmd.arg(output_path);

    let output = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("failed to launch ffmpeg")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr.lines().rev().take(20).collect::<Vec<_>>().join("\n");
        anyhow::bail!("ffmpeg failed to encode {}:\n{tail}", output_path.display());
    }
    Ok(())
}

/// Decodes `path` fully to 44.1kHz stereo `f32` samples in `[-1.0, 1.0]`,
/// resampling if the source isn't already at that rate and duplicating a
/// mono source to both channels.
fn load_stereo_44100(path: &Path) -> Result<[Vec<f32>; 2]> {
    let file = BufReader::new(
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?,
    );
    let source =
        Decoder::new(file).with_context(|| format!("failed to decode {}", path.display()))?;
    let channels = source.channels().max(1) as usize;
    let sample_rate = source.sample_rate().max(1);

    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut frame = vec![0f32; channels];
    let mut pos = 0usize;
    for sample in source {
        frame[pos] = sample as f32 / i16::MAX as f32;
        pos += 1;
        if pos < channels {
            continue;
        }
        pos = 0;
        if channels == 1 {
            left.push(frame[0]);
            right.push(frame[0]);
        } else {
            left.push(frame[0]);
            right.push(frame[1]);
        }
    }

    if sample_rate == MODEL_SAMPLE_RATE {
        Ok([left, right])
    } else {
        resample_to_44100(left, right, sample_rate)
    }
}

fn resample_to_44100(left: Vec<f32>, right: Vec<f32>, rate_in: u32) -> Result<[Vec<f32>; 2]> {
    let len = left.len();
    let channel_data = [left, right];
    let input = SequentialSliceOfVecs::new(&channel_data, 2, len)
        .map_err(|e| anyhow::anyhow!("failed to wrap input audio for resampling: {e}"))?;

    let mut resampler = Fft::<f32>::new_custom(
        rate_in as usize,
        MODEL_SAMPLE_RATE as usize,
        4096,
        1,
        2,
        WindowFunction::BlackmanHarris2,
        FixedSync::Both,
    )
    .map_err(|e| anyhow::anyhow!("failed to build resampler: {e}"))?;

    let output = resampler
        .process_all(&input, len, None)
        .map_err(|e| anyhow::anyhow!("resampling failed: {e}"))?;
    let frames = output.frames();
    let interleaved = output.take_data();

    let mut out_left = Vec::with_capacity(frames);
    let mut out_right = Vec::with_capacity(frames);
    for i in 0..frames {
        out_left.push(interleaved[i * 2]);
        out_right.push(interleaved[i * 2 + 1]);
    }
    Ok([out_left, out_right])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_input_file_fails_cleanly_instead_of_panicking() {
        let missing_in = Path::new("/nonexistent/definitely-not-a-real-file.wav");
        assert!(load_stereo_44100(missing_in).is_err());
    }

    #[test]
    fn write_stem_to_file_round_trips_through_wav() {
        let stem = [vec![0.0f32, 0.5, -0.5, 1.0], vec![0.0f32, -0.5, 0.5, -1.0]];
        let path = std::env::temp_dir().join(format!(
            "abyssal-cdg-vocals-test-write-{}.wav",
            std::process::id()
        ));
        write_stem_to_file(&stem, 44_100, &path).unwrap();

        let mut reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_rate, 44_100);
        let samples: Vec<f32> = reader.samples::<f32>().map(|s| s.unwrap()).collect();
        assert_eq!(samples, vec![0.0, 0.0, 0.5, -0.5, -0.5, 0.5, 1.0, -1.0]);

        let _ = std::fs::remove_file(&path);
    }

    /// End-to-end smoke test against the *real* ONNX model - not run by
    /// default (`cargo test`) since it needs the real ~65MB model file
    /// available (bundled resource or already cached - see
    /// `model_assets.rs`) and takes real CPU time to run inference.
    /// Doesn't judge separation quality (this uses a synthetic tone mix,
    /// not real music/vocals the model was trained on) - just that the
    /// full pipeline (model load, chunked STFT/ISTFT, overlap-add,
    /// subtraction) runs to completion on real weights without producing
    /// NaNs, silence, or a length mismatch. Run with:
    ///   cargo test --release vocals::tests::real_model_smoke_test -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_model_smoke_test() {
        let sample_rate = 44_100usize;
        let seconds = 12usize;
        let n = sample_rate * seconds;
        let mut left = Vec::with_capacity(n);
        let mut right = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f32 / sample_rate as f32;
            // A low tone (stand-in for instrumentation) plus a higher,
            // amplitude-modulated tone (stand-in for a vocal-like element)
            // - not real music, just enough spectral structure to exercise
            // every stage of the pipeline meaningfully.
            let bass = (2.0 * std::f32::consts::PI * 110.0 * t).sin() * 0.3;
            let lead = (2.0 * std::f32::consts::PI * 660.0 * t).sin()
                * (0.5 + 0.5 * (2.0 * std::f32::consts::PI * 3.0 * t).sin())
                * 0.3;
            left.push(bass + lead);
            right.push(bass * 0.9 + lead * 1.1);
        }

        let mut separator = load_separator().expect("model should load");
        let mix = [left.clone(), right.clone()];
        let result = separator
            .separate(&mix, |p| eprintln!("progress: {:.1}%", p * 100.0))
            .expect("separation should succeed");

        assert_eq!(result.instrumental[0].len(), n);
        assert_eq!(result.vocals[0].len(), n);

        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        let mix_rms = rms(&left);
        let inst_rms = rms(&result.instrumental[0]);
        let vox_rms = rms(&result.vocals[0]);
        eprintln!(
            "mix RMS: {mix_rms:.4}, instrumental RMS: {inst_rms:.4}, vocals RMS: {vox_rms:.4}"
        );

        for ch in [
            &result.instrumental[0],
            &result.instrumental[1],
            &result.vocals[0],
            &result.vocals[1],
        ] {
            assert!(ch.iter().all(|v| v.is_finite()), "output contains NaN/Inf");
        }
        assert!(inst_rms > 1e-4, "instrumental stem is silent");
        assert!(vox_rms > 1e-4, "vocals stem is silent");
    }
}
