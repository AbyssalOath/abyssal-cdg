//! Waveform peak extraction for the fine-tuning timeline - decodes the
//! loaded audio file once into coarse min/max buckets, cheap enough to
//! redraw every frame (even while zoomed out over a whole song), so
//! dragging a bubble to align with a vocal onset has an actual visual cue
//! to align against instead of just eyeballing the numbers.
//!
//! The same decode pass also finds *onsets* (see [`OnsetDetector`]) - the
//! moments new sound starts - which timeline drags snap word/line starts
//! to.

use anyhow::{Context, Result};
use realfft::{RealFftPlanner, RealToComplex};
use rodio::{Decoder, Source};
use std::fs::File;
use std::io::BufReader;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

/// How many buckets per second of audio to keep - fine enough to still look
/// smooth at the timeline's highest zoom level (`timeline::MAX_PX_PER_SEC`),
/// coarse enough that a multi-minute song's whole waveform is a trivial
/// amount of memory and per-frame work to scan.
const BUCKETS_PER_SEC: f64 = 800.0;

/// Coarse min/max amplitude per time bucket, covering the whole file at a
/// fixed resolution independent of the source's sample rate/channel count.
pub struct Waveform {
    bucket_duration_secs: f64,
    /// (min, max) sample amplitude per bucket, within [-1.0, 1.0].
    peaks: Vec<(f32, f32)>,
    /// Onset times in seconds, ascending - see [`OnsetDetector`].
    onsets: Vec<f64>,
}

impl Waveform {
    /// Onsets (seconds, ascending) within `[from, to]`.
    pub fn onsets_in(&self, from: f64, to: f64) -> &[f64] {
        let a = self.onsets.partition_point(|&t| t < from);
        let b = self.onsets.partition_point(|&t| t <= to);
        &self.onsets[a..b.max(a)]
    }

    /// The onset nearest `t`, if one is within `max_dist` seconds.
    pub fn nearest_onset(&self, t: f64, max_dist: f64) -> Option<f64> {
        nearest(&self.onsets, t, max_dist)
    }

    fn bucket_range(&self, start_secs: f64, end_secs: f64) -> Range<usize> {
        let len = self.peaks.len();
        let start = ((start_secs / self.bucket_duration_secs).floor().max(0.0) as usize).min(len);
        let end = ((end_secs / self.bucket_duration_secs).ceil().max(0.0) as usize)
            .min(len)
            .max(start);
        start..end
    }

    /// Min/max amplitude across every bucket touching `[start_secs, end_secs)`,
    /// used to render one pixel column's peak regardless of the current zoom
    /// level (a single bucket at high zoom, potentially thousands of buckets
    /// collapsed together at low zoom).
    pub fn peak_in_range(&self, start_secs: f64, end_secs: f64) -> Option<(f32, f32)> {
        let range = self.bucket_range(start_secs, end_secs);
        if range.is_empty() {
            return None;
        }
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for &(mn, mx) in &self.peaks[range] {
            lo = lo.min(mn);
            hi = hi.max(mx);
        }
        Some((lo, hi))
    }
}

/// Decodes `path` fully and reduces it to coarse peaks - can take a
/// noticeable fraction of a second for a multi-minute file, so callers
/// should run this on a background thread rather than blocking the UI.
pub fn build_waveform(path: &Path) -> Result<Waveform> {
    let file = BufReader::new(
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?,
    );
    let source =
        Decoder::new(file).with_context(|| format!("failed to decode {}", path.display()))?;
    let channels = source.channels().max(1) as usize;
    let sample_rate = source.sample_rate().max(1) as f64;
    let frames_per_bucket = ((sample_rate / BUCKETS_PER_SEC).round() as usize).max(1);

    let mut peaks = Vec::new();
    let mut bucket_min = f32::INFINITY;
    let mut bucket_max = f32::NEG_INFINITY;
    let mut frames_in_bucket = 0usize;

    // Mix down to mono by averaging each interleaved frame, then bucket by
    // frame count (not raw sample count) so multi-channel files don't end
    // up with buckets `channels` times too short.
    let mut frame_acc = 0.0f32;
    let mut channel_pos = 0usize;
    let mut onsets = OnsetDetector::new(sample_rate);
    for sample in source {
        frame_acc += sample as f32 / i16::MAX as f32;
        channel_pos += 1;
        if channel_pos < channels {
            continue;
        }
        let frame_value = frame_acc / channels as f32;
        frame_acc = 0.0;
        channel_pos = 0;
        onsets.push(frame_value);

        bucket_min = bucket_min.min(frame_value);
        bucket_max = bucket_max.max(frame_value);
        frames_in_bucket += 1;
        if frames_in_bucket >= frames_per_bucket {
            peaks.push((bucket_min, bucket_max));
            bucket_min = f32::INFINITY;
            bucket_max = f32::NEG_INFINITY;
            frames_in_bucket = 0;
        }
    }
    if frames_in_bucket > 0 {
        peaks.push((bucket_min, bucket_max));
    }

    Ok(Waveform {
        bucket_duration_secs: frames_per_bucket as f64 / sample_rate,
        peaks,
        onsets: onsets.finish(),
    })
}

/// The value in sorted `times` nearest `t`, if within `max_dist`.
fn nearest(times: &[f64], t: f64, max_dist: f64) -> Option<f64> {
    let i = times.partition_point(|&x| x < t);
    [i.checked_sub(1), Some(i)]
        .into_iter()
        .flatten()
        .filter_map(|j| times.get(j).copied())
        .filter(|&x| (x - t).abs() <= max_dist)
        .min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs()))
}

/// Analysis hop: one onset-strength value per 10 ms.
const ONSET_HOP_SECS: f64 = 0.010;
/// FFT window, ~46 ms at 44.1 kHz (rounded up to a power of two).
const ONSET_WINDOW_SECS: f64 = 0.040;
/// Only this band counts toward onset strength - where sung vowels and
/// consonants sit, leaving out most of the kick drum below it and hi-hats/
/// cymbals above it, so word starts stand out more than the beat does.
const ONSET_BAND_HZ: (f64, f64) = (200.0, 4000.0);
/// Two onsets closer than this are one event (the stronger is kept).
const ONSET_MIN_GAP_SECS: f64 = 0.06;
/// Peak picking: a frame must be the maximum within this many frames on
/// either side...
const ONSET_PEAK_RADIUS: usize = 3;
/// ...and stand above the local average (over this many frames either
/// side) by this fraction of the song's typical strong onset.
const ONSET_MEAN_RADIUS: usize = 15;
const ONSET_THRESHOLD: f32 = 0.1;
/// SuperFlux-style max filter width (bins either side) - see
/// [`OnsetDetector::analyze_frame`].
const ONSET_MAX_FILTER_BINS: usize = 3;

/// Finds onsets - moments new sound starts - in a mono signal fed one
/// sample at a time: log-compressed spectral flux (how much the spectrum
/// *rose* since the previous 10 ms frame) over [`ONSET_BAND_HZ`], then
/// local maxima that stand clearly above their surroundings. It runs on
/// the full mix, so drum hits and chord changes inside the vocal band still
/// show up; it's a snapping aid, not a transcription.
pub struct OnsetDetector {
    sample_rate: f64,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    hop: usize,
    band: std::ops::Range<usize>,
    /// The last `window.len()` samples.
    ring: Vec<f32>,
    filled: usize,
    since_hop: usize,
    prev_mag: Vec<f32>,
    flux: Vec<f32>,
    scratch_in: Vec<f32>,
    scratch_out: Vec<realfft::num_complex::Complex32>,
}

impl OnsetDetector {
    pub fn new(sample_rate: f64) -> Self {
        let sample_rate = sample_rate.max(1.0);
        let n = ((sample_rate * ONSET_WINDOW_SECS) as usize)
            .next_power_of_two()
            .max(64);
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(n);
        let window = (0..n)
            .map(|i| {
                let x = std::f32::consts::PI * 2.0 * i as f32 / n as f32;
                0.5 - 0.5 * x.cos()
            })
            .collect();
        let bin_hz = sample_rate / n as f64;
        let lo = ((ONSET_BAND_HZ.0 / bin_hz).floor() as usize).max(1);
        let hi = ((ONSET_BAND_HZ.1 / bin_hz).ceil() as usize).clamp(lo + 1, n / 2 + 1);
        let scratch_out = fft.make_output_vec();
        Self {
            sample_rate,
            hop: ((sample_rate * ONSET_HOP_SECS).round() as usize).max(1),
            band: lo..hi,
            ring: vec![0.0; n],
            filled: 0,
            since_hop: 0,
            prev_mag: vec![0.0; hi - lo],
            flux: Vec::new(),
            scratch_in: vec![0.0; n],
            scratch_out,
            fft,
            window,
        }
    }

    pub fn push(&mut self, sample: f32) {
        let n = self.ring.len();
        self.ring[self.filled % n] = sample;
        self.filled += 1;
        self.since_hop += 1;
        if self.since_hop >= self.hop {
            self.since_hop = 0;
            self.analyze_frame();
        }
    }

    fn analyze_frame(&mut self) {
        let n = self.ring.len();
        // Oldest sample first (the ring's write position), windowed.
        let start = self.filled % n;
        for i in 0..n {
            self.scratch_in[i] = self.ring[(start + i) % n] * self.window[i];
        }
        if self
            .fft
            .process(&mut self.scratch_in, &mut self.scratch_out)
            .is_err()
        {
            self.flux.push(0.0);
            return;
        }
        let mags: Vec<f32> = self
            .band
            .clone()
            .map(|bin| (1.0 + 10.0 * self.scratch_out[bin].norm()).ln())
            .collect();
        // Until the window has filled once, the "rise" is just from the
        // zero padding - record nothing.
        let warmed_up = self.filled >= n + self.hop;
        let mut flux = 0.0f32;
        if warmed_up {
            // Compare each bin with the *loudest nearby bin* of the previous
            // frame (SuperFlux): energy smearing into neighbouring bins as
            // a note moves through the window, or a slight pitch wobble,
            // then doesn't count as new sound.
            let m = self.prev_mag.len();
            for (k, &mag) in mags.iter().enumerate() {
                let a = k.saturating_sub(ONSET_MAX_FILTER_BINS);
                let b = (k + ONSET_MAX_FILTER_BINS + 1).min(m);
                let reference = self.prev_mag[a..b].iter().copied().fold(0.0, f32::max);
                flux += (mag - reference).max(0.0);
            }
        }
        self.prev_mag = mags;
        self.flux.push(flux);
    }

    /// Time (seconds) of flux frame `k`: the middle of its window, which
    /// ends at sample `(k + 1) * hop`.
    fn frame_time(&self, k: usize) -> f64 {
        let end = ((k + 1) * self.hop) as f64;
        ((end - self.ring.len() as f64 / 2.0) / self.sample_rate).max(0.0)
    }

    /// Picks the onsets out of everything pushed so far.
    pub fn finish(self) -> Vec<f64> {
        let flux = &self.flux;
        if flux.len() < 3 {
            return Vec::new();
        }
        // Scale by a high percentile rather than the max, so one huge
        // transient doesn't make every other onset look insignificant -
        // but never below a quarter of the max, or a sparse track (a few
        // notes over near-silence) would scale its noise floor up to look
        // like onsets.
        let mut sorted: Vec<f32> = flux.iter().copied().filter(|v| *v > 0.0).collect();
        if sorted.is_empty() {
            return Vec::new();
        }
        sorted.sort_by(f32::total_cmp);
        let p98 = sorted[((sorted.len() - 1) as f32 * 0.98) as usize];
        let max = sorted[sorted.len() - 1];
        let scale = p98.max(max * 0.25).max(1e-6);
        let norm: Vec<f32> = flux.iter().map(|v| v / scale).collect();

        // Running sum for the local mean.
        let mut prefix = vec![0.0f32; norm.len() + 1];
        for (i, v) in norm.iter().enumerate() {
            prefix[i + 1] = prefix[i] + v;
        }
        let mut picked: Vec<(f64, f32)> = Vec::new();
        for k in 1..norm.len() {
            let v = norm[k];
            let a = k.saturating_sub(ONSET_PEAK_RADIUS);
            let b = (k + ONSET_PEAK_RADIUS + 1).min(norm.len());
            if norm[a..b].iter().any(|&w| w > v) {
                continue;
            }
            let ma = k.saturating_sub(ONSET_MEAN_RADIUS);
            let mb = (k + ONSET_MEAN_RADIUS + 1).min(norm.len());
            let mean = (prefix[mb] - prefix[ma]) / (mb - ma) as f32;
            if v < mean + ONSET_THRESHOLD {
                continue;
            }
            let t = self.frame_time(k);
            match picked.last_mut() {
                Some(last) if t - last.0 < ONSET_MIN_GAP_SECS => {
                    if v > last.1 {
                        *last = (t, v);
                    }
                }
                _ => picked.push((t, v)),
            }
        }
        picked.into_iter().map(|(t, _)| t).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_in_range_returns_none_for_an_empty_waveform() {
        let w = Waveform {
            bucket_duration_secs: 0.001,
            peaks: Vec::new(),
            onsets: Vec::new(),
        };
        assert_eq!(w.peak_in_range(0.0, 1.0), None);
    }

    #[test]
    fn peak_in_range_aggregates_across_covered_buckets() {
        let w = Waveform {
            bucket_duration_secs: 0.01, // 100 buckets/sec
            peaks: vec![(-0.1, 0.1), (-0.5, 0.2), (-0.2, 0.9), (-0.05, 0.05)],
            onsets: Vec::new(),
        };
        // Covers buckets 1 and 2 (each 0.01s wide): [0.01, 0.03).
        let (lo, hi) = w.peak_in_range(0.01, 0.03).unwrap();
        assert_eq!(lo, -0.5);
        assert_eq!(hi, 0.9);
    }

    #[test]
    fn peak_in_range_clamps_a_range_that_runs_past_the_end() {
        let w = Waveform {
            bucket_duration_secs: 0.01,
            peaks: vec![(-0.3, 0.4), (-0.1, 0.1)],
            onsets: Vec::new(),
        };
        let (lo, hi) = w.peak_in_range(0.0, 100.0).unwrap();
        assert_eq!(lo, -0.3);
        assert_eq!(hi, 0.4);
    }

    #[test]
    fn peak_in_range_returns_none_when_the_range_is_entirely_past_the_end() {
        let w = Waveform {
            bucket_duration_secs: 0.01,
            peaks: vec![(-0.3, 0.4)],
            onsets: Vec::new(),
        };
        assert_eq!(w.peak_in_range(5.0, 6.0), None);
    }

    /// Feeds `secs` of a signal into a detector at 44.1 kHz.
    fn detect(secs: f64, mut f: impl FnMut(f64) -> f32) -> Vec<f64> {
        let rate = 44_100.0;
        let mut d = OnsetDetector::new(rate);
        for i in 0..(secs * rate) as usize {
            d.push(f(i as f64 / rate));
        }
        d.finish()
    }

    /// A sung-ish note: 440 Hz + harmonics, 10 ms attack, held, then a
    /// 40 ms release (voices fade out rather than clicking off).
    fn note(t: f64, start: f64, len: f64) -> f32 {
        let dt = t - start;
        if !(0.0..len).contains(&dt) {
            return 0.0;
        }
        let env = (dt / 0.01).min(1.0).min((len - dt) / 0.04) as f32;
        let w = 2.0 * std::f64::consts::PI * 440.0 * t;
        env * 0.3 * (w.sin() + 0.5 * (2.0 * w).sin() + 0.25 * (3.0 * w).sin()) as f32
    }

    #[test]
    fn onsets_land_on_note_starts() {
        let starts = [0.5, 1.2, 2.0, 2.45];
        let found = detect(3.0, |t| starts.iter().map(|&s| note(t, s, 0.3)).sum());
        assert_eq!(found.len(), starts.len(), "{found:?}");
        for (f, s) in found.iter().zip(starts) {
            assert!((f - s).abs() < 0.03, "onset {f} vs note at {s}: {found:?}");
        }
    }

    #[test]
    fn onsets_ignore_a_steady_tone_and_low_noise() {
        // One held tone from 0.2s plus faint noise: one onset, at its start.
        let mut seed = 1u32;
        let found = detect(3.0, |t| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            note(t, 0.2, 10.0) + 0.002 * noise
        });
        assert_eq!(found.len(), 1, "{found:?}");
        assert!((found[0] - 0.2).abs() < 0.03, "{found:?}");
    }

    #[test]
    fn silence_has_no_onsets() {
        assert!(detect(1.0, |_| 0.0).is_empty());
    }

    #[test]
    fn nearest_onset_respects_the_distance_limit() {
        let w = Waveform {
            bucket_duration_secs: 0.01,
            peaks: Vec::new(),
            onsets: vec![1.0, 2.0, 2.3],
        };
        assert_eq!(w.nearest_onset(2.2, 0.15), Some(2.3));
        assert_eq!(w.nearest_onset(1.9, 0.15), Some(2.0));
        assert_eq!(w.nearest_onset(1.5, 0.15), None);
        assert_eq!(w.onsets_in(1.5, 2.3), &[2.0, 2.3]);
        assert!(w.onsets_in(5.0, 6.0).is_empty());
    }

    #[test]
    fn build_waveform_fails_cleanly_on_a_missing_file() {
        let missing = Path::new("/nonexistent/definitely-not-a-real-audio-file.mp3");
        assert!(build_waveform(missing).is_err());
    }
}
