//! Waveform peak extraction for the fine-tuning timeline - decodes the
//! loaded audio file once into coarse min/max buckets, cheap enough to
//! redraw every frame (even while zoomed out over a whole song), so
//! dragging a bubble to align with a vocal onset has an actual visual cue
//! to align against instead of just eyeballing the numbers.

use anyhow::{Context, Result};
use rodio::{Decoder, Source};
use std::fs::File;
use std::io::BufReader;
use std::ops::Range;
use std::path::Path;

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
}

impl Waveform {
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
    for sample in source {
        frame_acc += sample as f32 / i16::MAX as f32;
        channel_pos += 1;
        if channel_pos < channels {
            continue;
        }
        let frame_value = frame_acc / channels as f32;
        frame_acc = 0.0;
        channel_pos = 0;

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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_in_range_returns_none_for_an_empty_waveform() {
        let w = Waveform {
            bucket_duration_secs: 0.001,
            peaks: Vec::new(),
        };
        assert_eq!(w.peak_in_range(0.0, 1.0), None);
    }

    #[test]
    fn peak_in_range_aggregates_across_covered_buckets() {
        let w = Waveform {
            bucket_duration_secs: 0.01, // 100 buckets/sec
            peaks: vec![(-0.1, 0.1), (-0.5, 0.2), (-0.2, 0.9), (-0.05, 0.05)],
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
        };
        assert_eq!(w.peak_in_range(5.0, 6.0), None);
    }

    #[test]
    fn build_waveform_fails_cleanly_on_a_missing_file() {
        let missing = Path::new("/nonexistent/definitely-not-a-real-audio-file.mp3");
        assert!(build_waveform(missing).is_err());
    }
}
