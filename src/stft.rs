//! Short-time Fourier transform matching PyTorch's `torch.stft`/`torch.istft`
//! with `center=True`, a periodic Hann window, and no explicit inverse
//! `length` - the exact convention `audio-separator`'s MDX-Net separator
//! uses (see its `uvr_lib_v5/stft.py`). Bit-faithful reproduction of this
//! matters: the ONNX model's weights were trained against spectra produced
//! by this specific convention, so any drift here (window shape, padding,
//! normalization) would degrade separation quality even with the right
//! model and the right n_fft/hop/dim_f.
//!
//! Two things PyTorch does that are easy to get subtly wrong:
//! - Forward padding is `reflect`, not zero - the signal is mirrored by
//!   `n_fft/2` samples on each side before framing, not padded with silence.
//! - Inverse reconstruction is a windowed overlap-add normalized by the
//!   overlap-added *squared* window (the standard NOLA formula), not a
//!   simple sum - required here since `n_fft/hop = 6` isn't a power-of-two
//!   ratio that would make the Hann window trivially constant-overlap-add.

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

/// A single channel's STFT, one frame per column: `n_fft/2 + 1` bins.
pub type Spectrogram = Vec<Vec<Complex32>>;

pub struct Stft {
    n_fft: usize,
    hop: usize,
    window: Vec<f32>,
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
}

impl Stft {
    pub fn new(n_fft: usize, hop: usize) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(n_fft);
        let c2r = planner.plan_fft_inverse(n_fft);
        let window = hann_window_periodic(n_fft);
        Self {
            n_fft,
            hop,
            window,
            r2c,
            c2r,
        }
    }

    /// Forward STFT of one real-valued channel, `center=True`: reflect-pads
    /// by `n_fft/2` on each side, then frames/windows/FFTs with the given
    /// hop. `signal.len()` must be `> n_fft/2` (true for any real chunk this
    /// app ever feeds it - see `mdx.rs`'s minimum chunk size).
    pub fn forward(&self, signal: &[f32]) -> Spectrogram {
        let pad = self.n_fft / 2;
        let padded = reflect_pad(signal, pad);
        let num_frames = 1 + (padded.len() - self.n_fft) / self.hop;

        let mut frames = Vec::with_capacity(num_frames);
        let mut scratch_in = vec![0f32; self.n_fft];
        let mut scratch_out = self.r2c.make_output_vec();

        for i in 0..num_frames {
            let start = i * self.hop;
            for j in 0..self.n_fft {
                scratch_in[j] = padded[start + j] * self.window[j];
            }
            self.r2c
                .process(&mut scratch_in, &mut scratch_out)
                .expect("realfft forward FFT failed");
            frames.push(scratch_out.clone());
        }
        frames
    }

    /// Inverse STFT, `center=True`: windowed overlap-add normalized by the
    /// overlap-added squared window, then crops the `n_fft/2` reflect-pad
    /// back off each end. Output length is `hop * (frames.len() - 1)`.
    pub fn inverse(&self, frames: &[Vec<Complex32>]) -> Vec<f32> {
        let num_frames = frames.len();
        assert!(num_frames >= 1, "inverse STFT needs at least one frame");
        let padded_len = self.n_fft + self.hop * (num_frames - 1);
        let mut ola = vec![0f32; padded_len];
        let mut norm = vec![0f32; padded_len];

        let mut scratch_spec = self.c2r.make_input_vec();
        let mut scratch_time = vec![0f32; self.n_fft];

        for (i, frame) in frames.iter().enumerate() {
            scratch_spec.copy_from_slice(frame);
            // A real-valued time-domain signal's spectrum is mathematically
            // required to have a purely real DC bin and Nyquist bin -
            // `realfft` enforces this strictly and errors otherwise, where
            // PyTorch's `istft` just silently tolerates/discards a stray
            // imaginary part there. A model's raw output isn't guaranteed
            // to respect this exactly (it has no way to know), so this
            // zeroes those two bins' imaginary components to match PyTorch's
            // effective behavior rather than reject otherwise-fine output.
            scratch_spec[0].im = 0.0;
            if let Some(nyquist) = scratch_spec.last_mut() {
                nyquist.im = 0.0;
            }
            self.c2r
                .process(&mut scratch_spec, &mut scratch_time)
                .expect("realfft inverse FFT failed");
            let start = i * self.hop;
            for j in 0..self.n_fft {
                // realfft's inverse is unnormalized (an FFTW-style round
                // trip scales by n_fft), so divide it back out here.
                let sample = scratch_time[j] / self.n_fft as f32;
                ola[start + j] += sample * self.window[j];
                norm[start + j] += self.window[j] * self.window[j];
            }
        }

        for (s, n) in ola.iter_mut().zip(norm.iter()) {
            if *n > 1e-11 {
                *s /= n;
            }
        }

        let pad = self.n_fft / 2;
        ola[pad..padded_len - pad].to_vec()
    }
}

/// `torch.hann_window(n, periodic=True)` - the DFT-even Hann window used
/// for overlap-add (first sample is 0, the symmetric endpoint isn't
/// repeated), as opposed to the "symmetric" Hann window used for e.g. FIR
/// filter design.
fn hann_window_periodic(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos())
        .collect()
}

/// `numpy.pad(x, pad, mode="reflect")` / PyTorch's default STFT pad mode:
/// mirrors around the edge sample without repeating it, e.g. padding
/// `[a, b, c, d, e]` by 2 gives `[c, b, a, b, c, d, e, d, c]`.
fn reflect_pad(x: &[f32], pad: usize) -> Vec<f32> {
    let n = x.len();
    assert!(
        pad < n,
        "reflect padding of {pad} needs a signal longer than {pad} samples, got {n}"
    );
    let mut out = Vec::with_capacity(n + 2 * pad);
    out.extend((1..=pad).rev().map(|i| x[i]));
    out.extend_from_slice(x);
    out.extend((0..pad).map(|i| x[n - 2 - i]));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(a: &[f32], b: &[f32], tol: f32) {
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            assert!(
                (x - y).abs() < tol,
                "index {i}: {x} vs {y} (diff {})",
                (x - y).abs()
            );
        }
    }

    #[test]
    fn hann_window_periodic_starts_at_zero_and_is_not_symmetric_endpoint_repeated() {
        let w = hann_window_periodic(8);
        assert_eq!(w.len(), 8);
        assert!(w[0].abs() < 1e-6);
        // Symmetric Hann (numpy.hanning(8)) would have w[0] == w[7] == 0;
        // periodic Hann's last sample is *not* 0 - that's the whole point.
        assert!(w[7] > 0.05);
    }

    #[test]
    fn reflect_pad_matches_numpy_reflect_convention() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        let padded = reflect_pad(&x, 2);
        assert_eq!(padded, vec![3.0, 2.0, 1.0, 2.0, 3.0, 4.0, 5.0, 4.0, 3.0]);
    }

    #[test]
    fn round_trip_reconstructs_a_simple_sine_at_mdx_net_params() {
        // Same n_fft/hop MDX-Net uses (see mdx.rs), on a chunk short enough
        // to keep the test fast but long enough for several real frames.
        let n_fft = 6144;
        let hop = 1024;
        let len = hop * 20;
        let signal: Vec<f32> = (0..len)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 44100.0).sin() * 0.5)
            .collect();

        let stft = Stft::new(n_fft, hop);
        let spec = stft.forward(&signal);
        let recon = stft.inverse(&spec);

        assert_eq!(recon.len(), signal.len());
        assert_close(&recon, &signal, 1e-3);
    }

    #[test]
    fn round_trip_reconstructs_random_noise() {
        let n_fft = 6144;
        let hop = 1024;
        let len = hop * 20;
        // Deterministic pseudo-random noise - no need for a real RNG crate
        // dependency just for a test fixture.
        let mut state: u32 = 0x1234_5678;
        let signal: Vec<f32> = (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect();

        let stft = Stft::new(n_fft, hop);
        let spec = stft.forward(&signal);
        let recon = stft.inverse(&spec);

        assert_close(&recon, &signal, 1e-3);
    }

    #[test]
    fn inverse_tolerates_a_stray_imaginary_part_on_the_dc_and_nyquist_bins() {
        // A real ML model's raw output spectrum has no way to guarantee
        // the DC/Nyquist bins come out purely real (see the comment in
        // `inverse`) - this reproduces that with a deliberately "dirty"
        // spectrum and checks it doesn't panic and still reconstructs
        // sensibly, instead of only ever testing spectra our own `forward`
        // produced (which are always already clean).
        let n_fft = 6144;
        let hop = 1024;
        let len = hop * 4;
        let signal: Vec<f32> = (0..len)
            .map(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / 44100.0).sin() * 0.4)
            .collect();

        let stft = Stft::new(n_fft, hop);
        let mut spec = stft.forward(&signal);
        for frame in &mut spec {
            frame[0].im = 0.01;
            let last = frame.len() - 1;
            frame[last].im = 0.01;
        }

        let recon = stft.inverse(&spec);
        assert_eq!(recon.len(), signal.len());
        assert!(recon.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn frame_count_matches_expected_dim_t_for_a_full_mdx_net_chunk() {
        // chunk_size = hop * (segment_size - 1); MDX-Net expects exactly
        // `segment_size` (dim_t) frames back out of a full chunk.
        let n_fft = 6144;
        let hop = 1024;
        let segment_size = 256;
        let chunk_size = hop * (segment_size - 1);
        let signal = vec![0.0f32; chunk_size];

        let stft = Stft::new(n_fft, hop);
        let spec = stft.forward(&signal);
        assert_eq!(spec.len(), segment_size);
        assert_eq!(spec[0].len(), n_fft / 2 + 1);
    }
}
