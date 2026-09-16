# Architecture

Abyssal CDG Creator is a single-binary desktop app: a GUI shell (`main.rs`) driving a
set of pure-logic modules that turn timed lyrics into either a legacy `.cdg` file or a
modern `.mp4` video. This doc describes how the pieces fit together; see the README for
what the app actually does from a user's perspective.

## Module map

```
main.rs      GUI (eframe/egui): loads audio, owns app state, wires buttons/shortcuts
             to the modules below, and hosts the live preview and the fine-tuning
             timeline widget.
audio.rs     Audio decode/playback (rodio + symphonia) and the wall-clock
             play/pause/resume/seek state machine used for tap-to-time.
lyrics.rs    The data model: LyricLine (raw, user-entered - start, optional explicit
             end, per-word start/end overrides) and TimedLine (resolved
             start/end/sing_end window). Word-highlight timing, countdown-window
             math, timecode format/parse, and start/end overlap validation all live
             here, shared by every renderer and by main.rs's tap/timeline/manual-entry
             code paths.
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
timeline.rs  Pure time<->pixel mapping, zoom/drag bounds, and drag-mode
             classification for the fine-tuning timeline - no egui dependency, so
             it's unit-tested directly; main.rs owns the actual widget/painting/
             interaction glue built on top of it.
vocals.rs    Shells out to the `audio-separator` CLI (a separate Python package,
             not bundled) to produce an instrumental copy of the loaded audio, for
             the "Export instrumental audio" button and the video export's "Remove
             vocals" checkbox.
```

## Data flow

1. The user pastes lyrics or loads a file (`formats.rs` auto-detects LRC/UltraStar/KOK
   vs. plain text) -> a `Vec<LyricLine>`. Each line has raw text, an optional `start`
   time, an optional explicit `sing_end_override`, a `Singer`, and per-word
   `word_overrides`/`word_end_overrides` (start and end, independently).
2. The user can fill in `start`/`sing_end_override` three ways, all converging on the
   same fields so they stay interchangeable: tapping along with playback (`main.rs`
   fills them in from `AudioPlayer::position()`, alternating start then end per line -
   see `TapPhase`), dragging a bubble/edge on the fine-tuning timeline (`timeline.rs`'s
   drag math resolves to a `(start, end)` pair main.rs writes back), or typing a
   `MM:SS.CC` timecode directly into the timing table. All three routes validate
   through `lyrics::check_start_change`/`check_end_change` first (a start/end can't
   cross a neighboring line's *explicitly set* time), and the timeline's own drag
   bounds separately prevent an in-progress drag from producing an invalid value at
   all rather than rejecting it after the fact.
3. At export/preview time, `lyrics::resolve_timing` turns `Vec<LyricLine>` into
   `Vec<TimedLine>`: it sorts by `start`, chains each line's `end` to the next line's
   `start` (or to the audio's total duration for the last line), and derives each
   line's `sing_end` from `sing_end_override` if set, else an estimate from word count.
4. `lyrics::word_timings` spreads word-highlight times across `[start, sing_end)`,
   proportional to character count, unless a word has a manual start/end override -
   an override can open a gap (the wipe holds at that word until the next one starts)
   but never a hard wall against overlapping the *next* word's territory, since
   `current_line_wipe_fraction` always hands off at the next word's own start
   regardless of what an earlier word's end says.
5. `export::render_cdg` and `video::render_video` both consume the same
   `&[TimedLine]` and the same `word_timings`/`countdown_window`/`blank_sung_lines`/
   `hide_upcoming_lines` helpers, so a `.cdg` export and an `.mp4` export of the same
   song always agree on *when* things happen - they only differ in *how* they're drawn
   (CDG's fixed 300x216 tile canvas vs. an arbitrary-resolution RGB frame buffer).
6. `main.rs`'s live preview is a *third* consumer of the same `TimedLine`/`word_timings`/
   `countdown_window` data (using `video.rs`'s multi-line block grouping, not the CDG
   layout - see the README's "Live preview vs. the exported file" section). The
   fine-tuning timeline is a *fourth* consumer, plus a *producer*: it reads the
   resolved timing to position bubbles, and writes back into the same `LyricLine`
   fields when a bubble is dragged - see `TimelineDrag`/`DragSession` in `main.rs`/
   `timeline.rs`.

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
encode that can take anywhere from several seconds to a couple of minutes. Instrumental
export (`vocals::remove_vocals_to_file`, via `start_vocal_removal_export`) follows the
same background-thread-plus-polled-`Arc<Mutex<...>>` pattern (`VocalRemovalExportHandle`/
`poll_vocal_removal_export`), just without a progress fraction - `audio-separator`
doesn't expose one, so the UI shows an indeterminate spinner instead. When "Remove
vocals" is checked for a video export, separation runs *inside* the video-export
thread first (into a throwaway temp WAV), before `video::render_video` is called with
that file instead of the original audio - one background thread, one status/progress
handle, not two independent operations to coordinate. Audio playback itself runs on
`rodio`'s own output thread; `AudioPlayer` only tracks a wall-clock position
(`PlaybackClock`) on the main thread, deliberately decoupled from the audio device so
it can be unit-tested without real hardware.

## External subprocess dependencies

Two features shell out to a separate command-line tool that must already be installed
and on `PATH` - neither is a build-time dependency, and neither is bundled with the app:

- **`ffmpeg`** (`video.rs::check_ffmpeg_available`) - required for `.mp4` video export.
  Frames are piped to it over stdin as raw RGB24, muxed with the loaded audio file.
- **`audio-separator`** (`vocals.rs::check_audio_separator_available`) - required for
  vocal removal. It's a Python package (see the README's "Removing vocals" section);
  this app only ever invokes its CLI with a fixed argument list (never through a shell -
  see `SECURITY.md`) and reads back the resulting file from a throwaway temp directory
  it cleans up afterward.

Both checks follow the same shape: try running the tool with a harmless flag
(`-version`/`--version`), and if that fails, return a clear, actionable error (what to
install and how) rather than a raw "command not found" or a silent failure later.

## Testing strategy

`cdg.rs`, `font.rs`, `lyrics.rs`, `formats.rs`, `export.rs`, `video.rs`, and
`timeline.rs` have no GUI/audio dependency and are unit-tested directly (`cargo test`,
no display or audio device required). `audio.rs`'s `PlaybackClock` (the play/pause/
resume/seek math) is tested the same way, decoupled from the real `rodio`/`symphonia`
calls it wraps. `vocals.rs`'s output-file-resolution logic (`find_output_file`) is
tested directly against a real temp directory; the actual `audio-separator` subprocess
invocation is not exercised in the test suite (same as `ffmpeg` in `video.rs`) since it
needs the external tool installed - both are meant to be verified manually. `main.rs`
(GUI wiring, including the timeline widget's painting/interaction code built on top of
`timeline.rs`) has no automated tests - it's a thin layer that calls into the tested
modules above, verified manually by running the app.
