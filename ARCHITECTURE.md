# Architecture

Abyssal CDG Creator is a single-binary desktop app: a GUI shell (`main.rs`) driving a
set of pure-logic modules that turn timed lyrics into either a legacy `.cdg` file or a
modern `.mp4` video. This doc describes how the pieces fit together; see the README for
what the app actually does from a user's perspective.

## Module map

```
main.rs      GUI (eframe/egui): loads audio, owns app state, wires buttons/shortcuts
             to the modules below, and hosts the live preview.
audio.rs     Audio decode/playback (rodio + symphonia) and the wall-clock
             play/pause/resume/seek state machine used for tap-to-time.
lyrics.rs    The data model: LyricLine (raw, user-entered) and TimedLine (resolved
             start/end/sing_end window). Word-highlight timing and countdown-window
             math live here, shared by every renderer.
formats.rs   Import/export for LRC, UltraStar, and KOK lyric files, plus format
             auto-detection. Produces LyricLine values, same as manual paste + tap.
cdg.rs       Low-level CD+Graphics packet encoder - the 24-byte packet format, tile
             blocks, CLUT loads, and timing-by-packet-position. No knowledge of
             lyrics; just a byte-stream writer.
font.rs      Renders characters into CDG's 6x12 tile format (and 2x/3x/... scaled
             variants) from the `font8x8` bitmap font.
export.rs    The ".cdg renderer": lays out lines/words/countdown dots on the CDG
             canvas using cdg.rs + font.rs, driven by lyrics.rs's timing.
video.rs     The "video renderer": an independent RGB24 frame renderer (via
             `ab_glyph` for anti-aliased text) piped into an `ffmpeg` subprocess,
             driven by the *same* lyrics.rs timing as export.rs.
```

## Data flow

1. The user pastes lyrics or loads a file (`formats.rs` auto-detects LRC/UltraStar/KOK
   vs. plain text) -> a `Vec<LyricLine>`. Each line has raw text, an optional `start`
   time, a `Singer`, and per-word manual overrides.
2. As the user taps along with playback, `main.rs` fills in each line's `start` from
   `AudioPlayer::position()`.
3. At export/preview time, `lyrics::resolve_timing` turns `Vec<LyricLine>` into
   `Vec<TimedLine>`: it sorts by `start`, chains each line's `end` to the next line's
   `start` (or to the audio's total duration for the last line), and derives each
   line's `sing_end` (an estimate of when singing actually stops, vs. just display time)
   from word count.
4. `lyrics::word_timings` spreads word-highlight times across `[start, sing_end)`,
   proportional to character count, unless a word has a manual override.
5. `export::render_cdg` and `video::render_video` both consume the same
   `&[TimedLine]` and the same `word_timings`/`countdown_window` helpers, so a `.cdg`
   export and an `.mp4` export of the same song always agree on *when* things happen -
   they only differ in *how* they're drawn (CDG's fixed 300x216 tile canvas vs. an
   arbitrary-resolution RGB frame buffer).
6. `main.rs`'s live preview is a *third* consumer of the same `TimedLine`/`word_timings`/
   `countdown_window` data, rendered with `egui` widgets instead of either exporter's
   pixel/tile format. It is deliberately not a decoder of the actual `.cdg` bytes -
   see the README's "Live preview vs. the exported file" section for why that's an
   intentional tradeoff, and what it means for accuracy.

Because steps 3-4 (the timing math) are shared by all three renderers, a fix or a change
to "how long is a countdown gap" or "how are word times estimated" only needs to happen
in `lyrics.rs` once. `lyrics::countdown_window_between` is a concrete example: it's the
one function that decides whether a "get ready" gap (either between two lines, or
between the title card and the first line) is long enough to show the countdown, and
`export.rs`, `video.rs`, and the live preview all call it rather than each
re-implementing the threshold check.

## The two export pipelines

- **`.cdg`** (`export.rs` + `cdg.rs` + `font.rs`) is a byte-stream builder: it writes
  packets in order, and *time* is represented purely by *how many packets* precede a
  given instruction (300 packets/sec, no separate timestamp field exists in the
  format). `CdgWriter::advance_to`/`pad_until` are the only ways time moves forward in
  this model - they never go backwards, so draw calls can be issued slightly out of
  strict chronological order without corrupting the stream, as long as each call's
  target time is monotonically non-decreasing overall.
- **`.mp4`** (`video.rs`) is a frame-by-frame renderer: for each output frame, it asks
  "what does the screen look like at time `t`?" and renders that from scratch into an
  RGB24 buffer, which it pipes to an `ffmpeg` subprocess over stdin (muxed with the
  loaded audio file, since CDG/raw video have no audio track of their own). This model
  has no "advance" step; each frame is independent, computed straight from `t` and the
  `TimedLine` list.

These are fundamentally different rendering strategies (packet-position-based vs.
frame-based), which is why they're separate modules rather than one renderer with two
output formats - but both are *fed* by the same timing data, per the section above.

## Threading

The GUI runs on the main thread (`eframe`'s event loop). Video export
(`video::render_video`) runs on a background thread (spawned in
`KaraokeApp::start_video_export`), reporting progress through an `Arc<AtomicU32>` and
its final result through an `Arc<Mutex<Option<Result<...>>>>`, both polled from
`update()` each frame via `poll_video_export`. This keeps the UI responsive during an
encode that can take anywhere from several seconds to a couple of minutes. Audio
playback itself runs on `rodio`'s own output thread; `AudioPlayer` only tracks a
wall-clock position (`PlaybackClock`) on the main thread, deliberately decoupled from
the audio device so it can be unit-tested without real hardware.

## Testing strategy

`cdg.rs`, `font.rs`, `lyrics.rs`, `formats.rs`, `export.rs`, and `video.rs` have no
GUI/audio dependency and are unit-tested directly (`cargo test`, no display or audio
device required). `audio.rs`'s `PlaybackClock` (the play/pause/resume/seek math) is
tested the same way, decoupled from the real `rodio`/`symphonia` calls it wraps.
`main.rs` (GUI wiring) has no automated tests - it's a thin layer that calls into the
tested modules above, verified manually by running the app.
