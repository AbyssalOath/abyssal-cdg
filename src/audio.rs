//! Audio loading + playback, used both to know the song's total length and
//! to let the user "tap along" to set lyric line timestamps while it plays,
//! and to seek to any position for fixing timing mistakes without having
//! to replay the whole song from the start.

use anyhow::{anyhow, Result};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Probe a media file for its duration in seconds, without decoding the
/// whole thing. Returns `None` if the container doesn't report a frame
/// count (rare, but some streamed/odd files omit it).
pub fn probe_duration_secs(path: &Path) -> Result<Option<f64>> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| anyhow!("no default audio track found in file"))?;

    let params = &track.codec_params;
    Ok(match (params.n_frames, params.time_base) {
        (Some(n_frames), Some(tb)) => {
            let t = tb.calc_time(n_frames);
            Some(t.seconds as f64 + t.frac)
        }
        _ => None,
    })
}

/// Pure wall-clock bookkeeping for "where are we in the song" - deliberately
/// decoupled from the actual audio device so the tricky play/pause/seek
/// interactions can be unit-tested without needing real audio hardware.
/// `AudioPlayer` delegates all its position math to this.
struct PlaybackClock {
    /// Position (seconds) as of the last seek/pause/stop.
    base_position: f64,
    /// Wall-clock instant the clock last started running, if currently
    /// playing (`None` means paused/stopped).
    running_since: Option<Instant>,
}

impl PlaybackClock {
    fn new() -> Self {
        Self {
            base_position: 0.0,
            running_since: None,
        }
    }

    fn position(&self) -> f64 {
        let elapsed = self
            .running_since
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        self.base_position + elapsed
    }

    fn is_running(&self) -> bool {
        self.running_since.is_some()
    }

    /// Start (or restart) running from `pos`.
    fn play_from(&mut self, pos: f64) {
        self.base_position = pos.max(0.0);
        self.running_since = Some(Instant::now());
    }

    fn pause(&mut self) {
        self.base_position = self.position();
        self.running_since = None;
    }

    fn resume(&mut self) {
        if self.running_since.is_none() {
            self.running_since = Some(Instant::now());
        }
    }

    fn stop(&mut self) {
        self.base_position = 0.0;
        self.running_since = None;
    }

    /// Jump to `pos`, preserving whether the clock was running or paused -
    /// this is the crux of scrubbing: dragging the seek bar while the song
    /// plays should keep playing from the new spot, while dragging it while
    /// paused should just move the marker without starting playback.
    fn seek(&mut self, pos: f64) {
        let was_running = self.is_running();
        self.base_position = pos.max(0.0);
        self.running_since = if was_running {
            Some(Instant::now())
        } else {
            None
        };
    }
}

/// Manages the single audio track being timed, with play/pause/resume/stop,
/// seeking, and a running playback-position clock (needed for tap-to-time).
pub struct AudioPlayer {
    _stream: OutputStream,
    stream_handle: OutputStreamHandle,
    sink: Option<Sink>,
    path: Option<PathBuf>,
    duration: Option<f64>,
    clock: PlaybackClock,
}

impl AudioPlayer {
    pub fn new() -> Result<Self> {
        let (stream, stream_handle) = OutputStream::try_default()?;
        Ok(Self {
            _stream: stream,
            stream_handle,
            sink: None,
            path: None,
            duration: None,
            clock: PlaybackClock::new(),
        })
    }

    pub fn load(&mut self, path: PathBuf) -> Result<()> {
        self.stop();
        let duration = probe_duration_secs(&path).unwrap_or(None);
        self.path = Some(path);
        self.duration = duration;
        Ok(())
    }

    pub fn file_name(&self) -> Option<String> {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
    }

    /// The loaded audio file's path, if any - needed to hand off to ffmpeg
    /// for video export (which muxes this file's audio into the output).
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn duration(&self) -> Option<f64> {
        self.duration
    }

    pub fn is_loaded(&self) -> bool {
        self.path.is_some()
    }

    pub fn is_playing(&self) -> bool {
        self.clock.is_running()
    }

    /// Current playback position in seconds.
    pub fn position(&self) -> f64 {
        let pos = self.clock.position();
        match self.duration {
            Some(d) => pos.min(d),
            None => pos,
        }
    }

    /// (Re)start playback from the beginning.
    pub fn play_from_start(&mut self) -> Result<()> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| anyhow!("no audio file loaded"))?;
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        let file = BufReader::new(File::open(&path)?);
        let source = Decoder::new(file)?;
        let sink = Sink::try_new(&self.stream_handle)?;
        sink.append(source);
        self.sink = Some(sink);
        self.clock.play_from(0.0);
        Ok(())
    }

    /// Jump to `pos` (seconds), clamped to the song's duration if known.
    /// Preserves play/pause state: seeking while playing keeps playing from
    /// the new spot; seeking while paused (or before playback has ever
    /// started) just moves the marker without making sound.
    ///
    /// Implementation note: this deliberately does *not* use rodio's
    /// `Sink::try_seek` - that call silently no-ops (returns `Ok(())`
    /// without actually moving anything) if the sink's internal sound
    /// counter reads zero, which we hit in practice. Instead we rebuild the
    /// pipeline from scratch each time and use `Source::skip_duration` to
    /// fast-forward past the audio before the target position. This is a
    /// little heavier per seek (it decodes-and-discards up to `pos`), but
    /// it's far more reliable across formats and doesn't depend on the
    /// container's seek-table quality - and decoding runs many times faster
    /// than real time, so it's still effectively instant.
    pub fn seek(&mut self, pos: f64) -> Result<()> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| anyhow!("no audio file loaded"))?;
        let pos = match self.duration {
            Some(d) => pos.clamp(0.0, d),
            None => pos.max(0.0),
        };
        let was_running = self.clock.is_running();

        if let Some(sink) = self.sink.take() {
            sink.stop();
        }

        let file = BufReader::new(File::open(&path)?);
        let source = Decoder::new(file)?.skip_duration(Duration::from_secs_f64(pos));
        let sink = Sink::try_new(&self.stream_handle)?;
        sink.append(source);
        if !was_running {
            // Avoid audibly starting playback if we were paused (or never started).
            sink.pause();
        }
        self.sink = Some(sink);
        self.clock.seek(pos);
        Ok(())
    }

    /// Resume from a paused state (no-op if nothing loaded or nothing paused).
    pub fn resume(&mut self) {
        if let Some(sink) = &self.sink {
            if sink.is_paused() {
                sink.play();
                self.clock.resume();
            }
        }
    }

    pub fn pause(&mut self) {
        if let Some(sink) = &self.sink {
            sink.pause();
        }
        self.clock.pause();
    }

    pub fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        self.clock.stop();
    }

    /// True once the sink has finished playing all queued audio on its own.
    pub fn finished_naturally(&self) -> bool {
        match &self.sink {
            Some(sink) => self.clock.is_running() && sink.empty(),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    const TOL: f64 = 0.05;

    #[test]
    fn starts_at_zero_not_running() {
        let clock = PlaybackClock::new();
        assert_eq!(clock.position(), 0.0);
        assert!(!clock.is_running());
    }

    #[test]
    fn play_from_advances_position() {
        let mut clock = PlaybackClock::new();
        clock.play_from(5.0);
        assert!(clock.is_running());
        sleep(Duration::from_millis(100));
        let pos = clock.position();
        assert!((pos - 5.1).abs() < TOL, "expected ~5.1, got {pos}");
    }

    #[test]
    fn pause_freezes_position() {
        let mut clock = PlaybackClock::new();
        clock.play_from(0.0);
        sleep(Duration::from_millis(100));
        clock.pause();
        assert!(!clock.is_running());
        let frozen = clock.position();
        sleep(Duration::from_millis(100));
        // Position must not advance further while paused.
        assert_eq!(clock.position(), frozen);
        assert!((frozen - 0.1).abs() < TOL, "expected ~0.1, got {frozen}");
    }

    #[test]
    fn resume_continues_from_paused_position_not_from_zero() {
        let mut clock = PlaybackClock::new();
        clock.play_from(10.0);
        sleep(Duration::from_millis(80));
        clock.pause();
        let paused_at = clock.position();
        clock.resume();
        assert!(clock.is_running());
        // Immediately after resuming, position should still be ~paused_at,
        // not reset to 10.0 or 0.0.
        let just_after = clock.position();
        assert!(
            (just_after - paused_at).abs() < TOL,
            "expected ~{paused_at}, got {just_after}"
        );
    }

    #[test]
    fn seek_while_running_keeps_running() {
        let mut clock = PlaybackClock::new();
        clock.play_from(0.0);
        clock.seek(42.0);
        assert!(clock.is_running());
        let pos = clock.position();
        assert!((pos - 42.0).abs() < TOL, "expected ~42.0, got {pos}");
    }

    #[test]
    fn seek_while_paused_stays_paused() {
        let mut clock = PlaybackClock::new();
        clock.play_from(0.0);
        clock.pause();
        clock.seek(17.0);
        assert!(!clock.is_running());
        assert_eq!(clock.position(), 17.0);
        sleep(Duration::from_millis(50));
        // Still frozen - seeking while paused shouldn't start the clock.
        assert_eq!(clock.position(), 17.0);
    }

    #[test]
    fn stop_resets_to_zero_and_not_running() {
        let mut clock = PlaybackClock::new();
        clock.play_from(30.0);
        clock.stop();
        assert!(!clock.is_running());
        assert_eq!(clock.position(), 0.0);
    }

    #[test]
    fn seek_never_goes_negative() {
        let mut clock = PlaybackClock::new();
        clock.play_from(5.0);
        clock.seek(-10.0);
        // Use a tolerance, not exact equality: some real wall-clock time
        // elapses between seek() and position() even in a tight test, and
        // the clock is still "running" (clamped to 0.0, not stopped).
        assert!(
            clock.position() < TOL,
            "expected ~0.0, got {}",
            clock.position()
        );
    }
}
