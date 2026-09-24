//! Native MDX-Net vocal/instrumental source separation via `ort` (ONNX
//! Runtime) - replaces the old `audio-separator` Python shell-out (see
//! `vocals.rs`).
//!
//! The algorithm here (STFT convention, chunked overlap-add, the
//! subtraction formula for the derived stem) is a bit-faithful port of
//! `audio_separator/separator/architectures/mdx_separator.py` and its
//! `uvr_lib_v5/stft.py`, reverse-engineered by reading that source
//! directly - not reimplemented from a general understanding of MDX-Net,
//! since matching this exact reference matters more than matching "an"
//! MDX-Net implementation. Every model's own parameters below were
//! verified against the actual `.onnx` file's own hash (UVR's own
//! MD5-of-last-10000KiB scheme - confirmed directly by reproducing
//! [`MdxModel::InstHq3`]'s already-known-correct hash before trusting the
//! same method for [`MdxModel::KimVocal2`]), looked up in
//! `TRvlvr/application_data`'s `mdx_model_data/model_data_new.json`, not
//! taken from a secondary description of either model. `hop_length` is
//! the one exception - not in that JSON at all, because (confirmed
//! directly in `audio_separator`'s own source, `mdx_separator.py`) UVR
//! hard-codes it to 1024 for every model it publishes, rather than it
//! being a real per-model value.

use crate::stft::Stft;
use anyhow::{anyhow, Result};
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use realfft::num_complex::Complex32;
use std::path::Path;

/// UVR hard-codes this for every MDX-Net model it publishes (see the
/// module docs) - not a per-model value despite living next to ones that
/// are.
const HOP_LENGTH: usize = 1024;

// --- Arch-level defaults from audio-separator's `MDXParams` (not
// model-specific, but this app doesn't expose them as user settings). ---
const OVERLAP: f32 = 0.25;

/// Which stem an [`MdxModel`] directly outputs - the other one is always
/// derived by subtraction from the mix (see [`MdxSeparator::separate`]),
/// never a second model pass. Every public UVR-MDX-NET release is one or
/// the other, never both directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrimaryStem {
    Instrumental,
    Vocals,
}

/// One model's own verified parameters - see the module docs for how
/// these were sourced.
#[derive(Clone, Copy, Debug)]
struct ModelParams {
    n_fft: usize,
    dim_f: usize,
    /// `2 ** mdx_dim_t_set` in the model's own data.
    segment_size: usize,
    compensate: f32,
    primary_stem: PrimaryStem,
}

/// A selectable vocal-isolation model - currently just used to isolate
/// vocals before auto-align (see `main.rs`'s `start_word_alignment`), not
/// the general "Instrumental audio"/"Vocals audio" export checkboxes or
/// video export's "Remove vocals" (those stay on [`MdxModel::InstHq3`]
/// unconditionally - see `vocals::load_separator`). A deliberately small,
/// curated set, not an open picker: each model's STFT/chunking math is
/// tied to its own exact [`ModelParams`], so an arbitrary `.onnx` dropped
/// in without a verified entry here would risk silently wrong output, not
/// just fail loudly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MdxModel {
    /// The long-standing default - outputs the instrumental stem
    /// directly, vocals derived by subtraction. Good all-around
    /// separation quality; the one this app has always used.
    #[default]
    InstHq3,
    /// Outputs the *vocals* stem directly instead (instrumental becomes
    /// the derived one) - widely regarded as one of the cleanest MDX-Net
    /// models specifically for vocal isolation, at some cost to
    /// instrumental cleanliness. Worth trying if auto-align's results
    /// seem off and you suspect the isolated vocal stem it's aligning
    /// against isn't clean enough - not a strict upgrade over Inst HQ 3,
    /// a different trade-off.
    KimVocal2,
}

impl MdxModel {
    pub const ALL: [MdxModel; 2] = [Self::InstHq3, Self::KimVocal2];

    pub fn label(self) -> &'static str {
        match self {
            Self::InstHq3 => "Inst HQ 3 (default)",
            Self::KimVocal2 => "Kim Vocal 2 (prioritizes clean vocals)",
        }
    }

    pub fn filename(self) -> &'static str {
        match self {
            Self::InstHq3 => "UVR-MDX-NET-Inst_HQ_3.onnx",
            Self::KimVocal2 => "Kim_Vocal_2.onnx",
        }
    }

    pub fn download_url(self) -> &'static str {
        match self {
            Self::InstHq3 => "https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models/UVR-MDX-NET-Inst_HQ_3.onnx",
            Self::KimVocal2 => "https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models/Kim_Vocal_2.onnx",
        }
    }

    fn params(self) -> ModelParams {
        match self {
            Self::InstHq3 => ModelParams {
                n_fft: 6144,
                dim_f: 3072,
                segment_size: 256,
                compensate: 1.022,
                primary_stem: PrimaryStem::Instrumental,
            },
            Self::KimVocal2 => ModelParams {
                n_fft: 7680,
                dim_f: 3072,
                segment_size: 256,
                compensate: 1.009,
                primary_stem: PrimaryStem::Vocals,
            },
        }
    }
}

/// Both stems of a separated stereo mix, same sample rate/length as the
/// input.
pub struct Separated {
    /// Music with vocals removed.
    pub instrumental: [Vec<f32>; 2],
    /// Isolated vocals.
    pub vocals: [Vec<f32>; 2],
}

pub struct MdxSeparator {
    session: Session,
    stft: Stft,
    params: ModelParams,
}

impl MdxSeparator {
    pub fn load(model_path: &Path, model: MdxModel) -> Result<Self> {
        crate::onnxrt::ensure_loaded()?;
        let mut builder = Session::builder()
            .map_err(|e| anyhow!("failed to create an ONNX Runtime session builder: {e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow!("failed to configure ONNX Runtime session: {e}"))?;
        let session = builder
            .commit_from_file(model_path)
            .map_err(|e| anyhow!("failed to load ONNX model {}: {e}", model_path.display()))?;
        let params = model.params();
        Ok(Self {
            session,
            stft: Stft::new(params.n_fft, HOP_LENGTH),
            params,
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
        let primary: [Vec<f32>; 2] = [
            demixed[0].iter().map(|v| v * peak).collect(),
            demixed[1].iter().map(|v| v * peak).collect(),
        ];

        // The derived stem, by subtraction, not a second model pass -
        // audio-separator's default (`invert_using_spec=False`) path:
        // `secondary = -primary*compensate + mix` using the *normalized*
        // mix, not the peak-rescaled one. This looks like a scale
        // mismatch unless peak <= 0.9 (in which case normalize was a
        // no-op) - that's genuinely how the reference implementation
        // behaves by default, so it's replicated as-is rather than
        // "corrected" into different output.
        let compensate = self.params.compensate;
        let derived: [Vec<f32>; 2] = [
            (0..len)
                .map(|i| -primary[0][i] * compensate + norm_mix[0][i])
                .collect(),
            (0..len)
                .map(|i| -primary[1][i] * compensate + norm_mix[1][i])
                .collect(),
        ];

        // Which array is "instrumental" vs "vocals" depends on which one
        // the loaded model actually outputs directly - see
        // `PrimaryStem`/`MdxModel::KimVocal2`'s own docs.
        Ok(match self.params.primary_stem {
            PrimaryStem::Instrumental => Separated {
                instrumental: primary,
                vocals: derived,
            },
            PrimaryStem::Vocals => Separated {
                instrumental: derived,
                vocals: primary,
            },
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
        let trim = self.params.n_fft / 2;
        let chunk_size = HOP_LENGTH * (self.params.segment_size - 1);
        let gen_size = chunk_size - 2 * trim;

        let len = mix[0].len();
        let pad = gen_size + trim - (len % gen_size);
        let padded_len = trim + len + pad;
        let mut padded = [vec![0f32; padded_len], vec![0f32; padded_len]];
        for ch in 0..2 {
            padded[ch][trim..trim + len].copy_from_slice(&mix[ch]);
        }

        let step = ((1.0 - OVERLAP) * chunk_size as f32) as usize;
        let mut result = [vec![0f32; padded_len], vec![0f32; padded_len]];
        let mut divider = [vec![0f32; padded_len], vec![0f32; padded_len]];

        let window = hann_window(chunk_size);
        let total_steps = padded_len.div_ceil(step).max(1);

        let mut start = 0usize;
        let mut step_idx = 0usize;
        while start < padded_len {
            let end = (start + chunk_size).min(padded_len);
            let actual_len = end - start;

            let mut chunk = [vec![0f32; chunk_size], vec![0f32; chunk_size]];
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
        // pad added to reach a multiple of gen_size, leaving exactly the
        // original mix length.
        let mut out = [Vec::with_capacity(len), Vec::with_capacity(len)];
        for ch in 0..2 {
            out[ch].extend_from_slice(&result[ch][trim..trim + len]);
        }
        Ok(out)
    }

    /// One chunk through STFT -> zero low bins -> ONNX model -> ISTFT,
    /// matching `MDXSeparator.run_model` (denoise disabled - this app
    /// doesn't expose it as a setting, matching audio-separator's default).
    fn run_model(&mut self, chunk: &[Vec<f32>; 2]) -> Result<[Vec<f32>; 2]> {
        let dim_f = self.params.dim_f;
        let spec_l = self.stft.forward(&chunk[0]);
        let spec_r = self.stft.forward(&chunk[1]);
        let frames = spec_l.len();

        // Interleave channels as [L_re, L_im, R_re, R_im] and truncate the
        // frequency axis to dim_f, matching `STFT.__call__`.
        let mut input = ndarray::Array4::<f32>::zeros((1, 4, dim_f, frames));
        for (t, (l, r)) in spec_l.iter().zip(spec_r.iter()).enumerate() {
            for f in 0..dim_f {
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
            out_shape.len() == 4 && out_shape[1] == 4 && out_shape[2] == dim_f,
            "unexpected model output shape {out_shape:?}, expected [_, 4, {dim_f}, _]"
        );
        let out_frames = out_shape[3];
        let stride_c = dim_f * out_frames;

        // Pad the frequency axis back from dim_f to n_bins with zeros,
        // split channels back into (re, im) pairs, and inverse-STFT each
        // channel - matches `STFT.inverse`.
        let n_bins = self.params.n_fft / 2 + 1;
        let mut out_l: Vec<Vec<Complex32>> = Vec::with_capacity(out_frames);
        let mut out_r: Vec<Vec<Complex32>> = Vec::with_capacity(out_frames);
        for t in 0..out_frames {
            let mut fl = vec![Complex32::new(0.0, 0.0); n_bins];
            let mut fr = vec![Complex32::new(0.0, 0.0); n_bins];
            for f in 0..dim_f {
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

    /// Locks in the verified parameters for every offered model (see the
    /// module docs for how each was sourced) - a silent edit to any of
    /// these would produce wrong (not necessarily crashing) separated
    /// audio, so they're worth a real regression test, not just trusting
    /// the source once and moving on.
    #[test]
    fn every_model_has_its_own_verified_parameters() {
        let inst_hq_3 = MdxModel::InstHq3.params();
        assert_eq!(inst_hq_3.n_fft, 6144);
        assert_eq!(inst_hq_3.dim_f, 3072);
        assert_eq!(inst_hq_3.segment_size, 256);
        assert!((inst_hq_3.compensate - 1.022).abs() < 1e-6);
        assert_eq!(inst_hq_3.primary_stem, PrimaryStem::Instrumental);

        let kim_vocal_2 = MdxModel::KimVocal2.params();
        assert_eq!(kim_vocal_2.n_fft, 7680);
        assert_eq!(kim_vocal_2.dim_f, 3072);
        assert_eq!(kim_vocal_2.segment_size, 256);
        assert!((kim_vocal_2.compensate - 1.009).abs() < 1e-6);
        assert_eq!(kim_vocal_2.primary_stem, PrimaryStem::Vocals);
    }

    #[test]
    fn chunk_gen_and_trim_sizes_match_the_verified_inst_hq_3_parameters() {
        let p = MdxModel::InstHq3.params();
        let trim = p.n_fft / 2;
        let chunk_size = HOP_LENGTH * (p.segment_size - 1);
        let gen_size = chunk_size - 2 * trim;
        assert_eq!(trim, 3072);
        assert_eq!(chunk_size, 261_120);
        assert_eq!(gen_size, 254_976);
    }
}
