//! Native MDX-Net vocal/instrumental source separation via `ort` (ONNX
//! Runtime) running the real UVR-MDX-NET-Inst_HQ_3 weights - replaces the
//! old `audio-separator` Python shell-out (see `vocals.rs`).
//!
//! The algorithm here (STFT convention, chunked overlap-add, the
//! subtraction formula for the secondary/vocals stem) is a bit-faithful
//! port of `audio_separator/separator/architectures/mdx_separator.py` and
//! its `uvr_lib_v5/stft.py`, reverse-engineered by reading that source
//! directly - not reimplemented from a general understanding of MDX-Net,
//! since matching this exact reference matters more than matching "an"
//! MDX-Net implementation. The model-specific parameters below were
//! verified against the actual `.onnx` file's own hash (UVR's own
//! MD5-of-last-10MB scheme), looked up in `TRvlvr/application_data`'s
//! `mdx_model_data/model_data_new.json`, not taken from a secondary
//! description of the model.

use crate::stft::Stft;
use anyhow::{anyhow, Result};
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use realfft::num_complex::Complex32;
use std::path::Path;

// --- UVR-MDX-NET-Inst_HQ_3-specific parameters ---
const N_FFT: usize = 6144;
const HOP_LENGTH: usize = 1024;
const DIM_F: usize = 3072;
/// `2 ** mdx_dim_t_set` where `mdx_dim_t_set = 8` in the model's own data.
const SEGMENT_SIZE: usize = 256;
const COMPENSATE: f32 = 1.022;

// --- Arch-level defaults from audio-separator's `MDXParams` (not
// model-specific, but this app doesn't expose them as user settings). ---
const OVERLAP: f32 = 0.25;

const TRIM: usize = N_FFT / 2; // 3072
const CHUNK_SIZE: usize = HOP_LENGTH * (SEGMENT_SIZE - 1); // 261120
const GEN_SIZE: usize = CHUNK_SIZE - 2 * TRIM; // 254976

/// Both stems of a separated stereo mix, same sample rate/length as the
/// input.
pub struct Separated {
    /// Primary stem - UVR-MDX-NET-Inst_HQ_3's primary stem is
    /// "Instrumental", i.e. this is the music with vocals removed.
    pub instrumental: [Vec<f32>; 2],
    /// Secondary stem, derived (not a second model pass) - see
    /// [`MdxSeparator::separate`].
    pub vocals: [Vec<f32>; 2],
}

pub struct MdxSeparator {
    session: Session,
    stft: Stft,
}

impl MdxSeparator {
    pub fn load(model_path: &Path) -> Result<Self> {
        crate::onnxrt::ensure_loaded()?;
        let mut builder = Session::builder()
            .map_err(|e| anyhow!("failed to create an ONNX Runtime session builder: {e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow!("failed to configure ONNX Runtime session: {e}"))?;
        let session = builder
            .commit_from_file(model_path)
            .map_err(|e| anyhow!("failed to load ONNX model {}: {e}", model_path.display()))?;
        Ok(Self {
            session,
            stft: Stft::new(N_FFT, HOP_LENGTH),
        })
    }

    /// Separates a 44.1kHz stereo mix (`mix[0]` = left, `mix[1]` = right,
    /// equal length) into instrumental and vocals. `on_progress` is called
    /// with `0.0..=1.0` as chunks complete - separation of a full song is
    /// the slow part of this feature (many chunked model inference calls),
    /// so this is a meaningful progress signal, unlike a single fixed
    /// pass/fail like the old Python shell-out gave.
    pub fn separate(
        &mut self,
        mix: &[Vec<f32>; 2],
        mut on_progress: impl FnMut(f32),
    ) -> Result<Separated> {
        let len = mix[0].len();
        assert_eq!(mix[1].len(), len, "left/right channel length mismatch");

        let peak = mix
            .iter()
            .flat_map(|c| c.iter())
            .fold(0f32, |m, &v| m.max(v.abs()));
        // audio-separator's `spec_utils.normalize`: only ever scales
        // *down* to a 0.9 peak, never amplifies a quiet mix (this app's
        // default `amplification_threshold` is effectively 0, matching
        // audio-separator's own CLI default).
        let scale = if peak > 0.9 { 0.9 / peak } else { 1.0 };
        let norm_mix: [Vec<f32>; 2] = [
            mix[0].iter().map(|v| v * scale).collect(),
            mix[1].iter().map(|v| v * scale).collect(),
        ];

        let demixed = self.demix(&norm_mix, &mut on_progress)?;
        // Rescale the (normalized-input) demix output back up to the
        // original scale - matches `source = self.demix(mix) * peak`.
        let instrumental: [Vec<f32>; 2] = [
            demixed[0].iter().map(|v| v * peak).collect(),
            demixed[1].iter().map(|v| v * peak).collect(),
        ];

        // Secondary stem by subtraction, not a second model pass -
        // audio-separator's default (`invert_using_spec=False`) path:
        // `secondary = -primary*compensate + mix` using the *normalized*
        // mix, not the peak-rescaled one. This looks like a scale
        // mismatch unless peak <= 0.9 (in which case normalize was a
        // no-op) - that's genuinely how the reference implementation
        // behaves by default, so it's replicated as-is rather than
        // "corrected" into different output.
        let vocals: [Vec<f32>; 2] = [
            (0..len)
                .map(|i| -instrumental[0][i] * COMPENSATE + norm_mix[0][i])
                .collect(),
            (0..len)
                .map(|i| -instrumental[1][i] * COMPENSATE + norm_mix[1][i])
                .collect(),
        ];

        Ok(Separated {
            instrumental,
            vocals,
        })
    }

    /// Chunked, overlap-added demix of one full (already peak-normalized)
    /// stereo mix - `audio_separator`'s `MDXSeparator.demix` (the
    /// `is_match_mix=False` path only; this app has no use for the
    /// spectral "raw_mix" the other branch produces).
    fn demix(
        &mut self,
        mix: &[Vec<f32>; 2],
        on_progress: &mut impl FnMut(f32),
    ) -> Result<[Vec<f32>; 2]> {
        let len = mix[0].len();
        let pad = GEN_SIZE + TRIM - (len % GEN_SIZE);
        let padded_len = TRIM + len + pad;
        let mut padded = [vec![0f32; padded_len], vec![0f32; padded_len]];
        for ch in 0..2 {
            padded[ch][TRIM..TRIM + len].copy_from_slice(&mix[ch]);
        }

        let step = ((1.0 - OVERLAP) * CHUNK_SIZE as f32) as usize;
        let mut result = [vec![0f32; padded_len], vec![0f32; padded_len]];
        let mut divider = [vec![0f32; padded_len], vec![0f32; padded_len]];

        let window = hann_window(CHUNK_SIZE);
        let total_steps = padded_len.div_ceil(step).max(1);

        let mut start = 0usize;
        let mut step_idx = 0usize;
        while start < padded_len {
            let end = (start + CHUNK_SIZE).min(padded_len);
            let actual_len = end - start;

            let mut chunk = [vec![0f32; CHUNK_SIZE], vec![0f32; CHUNK_SIZE]];
            for ch in 0..2 {
                chunk[ch][..actual_len].copy_from_slice(&padded[ch][start..end]);
            }

            let tar = self.run_model(&chunk)?;

            for ch in 0..2 {
                for i in 0..actual_len {
                    result[ch][start + i] += tar[ch][i] * window[i];
                    divider[ch][start + i] += window[i];
                }
            }

            step_idx += 1;
            on_progress((step_idx as f32 / total_steps as f32).min(1.0));
            start += step;
        }

        for ch in 0..2 {
            for i in 0..padded_len {
                if divider[ch][i] > 1e-11 {
                    result[ch][i] /= divider[ch][i];
                }
            }
        }

        // Crop the outer `trim` padding off both ends, then the trailing
        // pad added to reach a multiple of GEN_SIZE, leaving exactly the
        // original mix length.
        let mut out = [Vec::with_capacity(len), Vec::with_capacity(len)];
        for ch in 0..2 {
            out[ch].extend_from_slice(&result[ch][TRIM..TRIM + len]);
        }
        Ok(out)
    }

    /// One chunk through STFT -> zero low bins -> ONNX model -> ISTFT,
    /// matching `MDXSeparator.run_model` (denoise disabled - this app
    /// doesn't expose it as a setting, matching audio-separator's default).
    fn run_model(&mut self, chunk: &[Vec<f32>; 2]) -> Result<[Vec<f32>; 2]> {
        let spec_l = self.stft.forward(&chunk[0]);
        let spec_r = self.stft.forward(&chunk[1]);
        let frames = spec_l.len();

        // Interleave channels as [L_re, L_im, R_re, R_im] and truncate the
        // frequency axis to dim_f, matching `STFT.__call__`.
        let mut input = ndarray::Array4::<f32>::zeros((1, 4, DIM_F, frames));
        for (t, (l, r)) in spec_l.iter().zip(spec_r.iter()).enumerate() {
            for f in 0..DIM_F {
                // First 3 frequency bins are zeroed (low-frequency noise
                // suppression), matching `spek[:, :, :3, :] *= 0`.
                let (lre, lim, rre, rim) = if f < 3 {
                    (0.0, 0.0, 0.0, 0.0)
                } else {
                    (l[f].re, l[f].im, r[f].re, r[f].im)
                };
                input[[0, 0, f, t]] = lre;
                input[[0, 1, f, t]] = lim;
                input[[0, 2, f, t]] = rre;
                input[[0, 3, f, t]] = rim;
            }
        }

        let input_tensor =
            Tensor::from_array(input).map_err(|e| anyhow!("failed to build input tensor: {e}"))?;
        let outputs = self
            .session
            .run(ort::inputs!["input" => input_tensor])
            .map_err(|e| anyhow!("ONNX Runtime inference failed: {e}"))?;
        // The model has exactly one output; audio-separator itself
        // accesses it positionally (`session.run(...)[0]`) rather than by
        // name, so this does too instead of assuming a specific name.
        let (out_shape, out_data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("failed to read model output tensor: {e}"))?;
        let out_shape: Vec<usize> = out_shape.iter().map(|&d| d as usize).collect();
        anyhow::ensure!(
            out_shape.len() == 4 && out_shape[1] == 4 && out_shape[2] == DIM_F,
            "unexpected model output shape {out_shape:?}, expected [_, 4, {DIM_F}, _]"
        );
        let out_frames = out_shape[3];
        let stride_c = DIM_F * out_frames;

        // Pad the frequency axis back from dim_f to n_bins with zeros,
        // split channels back into (re, im) pairs, and inverse-STFT each
        // channel - matches `STFT.inverse`.
        let n_bins = N_FFT / 2 + 1;
        let mut out_l: Vec<Vec<Complex32>> = Vec::with_capacity(out_frames);
        let mut out_r: Vec<Vec<Complex32>> = Vec::with_capacity(out_frames);
        for t in 0..out_frames {
            let mut fl = vec![Complex32::new(0.0, 0.0); n_bins];
            let mut fr = vec![Complex32::new(0.0, 0.0); n_bins];
            for f in 0..DIM_F {
                let idx = |c: usize| c * stride_c + f * out_frames + t;
                fl[f] = Complex32::new(out_data[idx(0)], out_data[idx(1)]);
                fr[f] = Complex32::new(out_data[idx(2)], out_data[idx(3)]);
            }
            out_l.push(fl);
            out_r.push(fr);
        }

        Ok([self.stft.inverse(&out_l), self.stft.inverse(&out_r)])
    }
}

/// `numpy.hanning(n)` (the *symmetric* Hann window, not the periodic one
/// `stft.rs` uses) - `demix`'s per-chunk overlap-add window, matching
/// `np.hanning(chunk_size_actual)` in `MDXSeparator.demix`.
fn hann_window(n: usize) -> Vec<f32> {
    if n == 1 {
        return vec![1.0];
    }
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hann_window_is_symmetric_and_zero_at_both_ends() {
        let w = hann_window(9);
        assert_eq!(w.len(), 9);
        assert!(w[0].abs() < 1e-6);
        assert!(w[8].abs() < 1e-6);
        assert!((w[4] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn chunk_gen_and_trim_sizes_match_the_verified_model_parameters() {
        assert_eq!(N_FFT, 6144);
        assert_eq!(HOP_LENGTH, 1024);
        assert_eq!(DIM_F, 3072);
        assert_eq!(SEGMENT_SIZE, 256);
        assert_eq!(TRIM, 3072);
        assert_eq!(CHUNK_SIZE, 261_120);
        assert_eq!(GEN_SIZE, 254_976);
    }
}
