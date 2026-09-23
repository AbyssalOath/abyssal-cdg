# Architecture

Abyssal CDG Creator is a single-binary desktop app: a GUI shell (`main.rs`) driving a
set of pure-logic modules that turn timed lyrics into a legacy `.cdg` file, a modern
`.mp4` video, and/or a portable `.lrc`/UltraStar `.txt` lyrics file. This doc describes
how the pieces fit together; see the README for what the app actually does from a
user's perspective.

## Module map

```
main.rs      GUI (eframe/egui): loads audio, owns app state, wires buttons/shortcuts
             to the modules below, and hosts the live preview and the fine-tuning
             timeline widget. Also owns the undo/redo history (a per-frame diff of
             the undoable subset of app state - see "Undo/redo" below) and the
             drag-and-drop/recent-files/crash-recovery glue.
audio.rs     Audio decode/playback (rodio + symphonia) and the wall-clock
             play/pause/resume/seek state machine used for tap-to-time.
waveform.rs  Decodes the loaded audio into coarse min/max peak buckets (background
             thread), drawn behind the timeline's bubbles as a visual aid for
             lining up a bubble against an actual vocal onset.
lyrics.rs    The data model: LyricLine (raw, user-entered - start, optional explicit
             end, per-word start/end overrides) and TimedLine (resolved
             start/end/sing_end window). Word-highlight timing, countdown-window
             math, timecode format/parse, and start/end overlap validation all live
             here, shared by every renderer and by main.rs's tap/timeline/manual-entry
             code paths. Also strips Genius-style `[Verse 1]`/`[Chorus]` section
             markers when parsing pasted lyrics. BackingVocal/TimedBackingVocal
             (a second vocalist echoing part of a line) are embedded on the host
             LyricLine/TimedLine rather than being independent top-level lines -
             their own timing is bounded within their host's window, so the
             sequential, non-overlapping TimedLine list itself never changes.
formats.rs   Import *and* export for LRC, UltraStar, and KOK lyric files, plus
             format auto-detection. Import produces LyricLine values, same as
             manual paste + tap; export is the direct inverse, consuming the same
             `&[TimedLine]` the CDG/video renderers do.
project.rs   Save/load for `.abyzl` project files (JSON, via serde) - everything
             needed to resume a session (lyrics, timing, colors, audio path, video
             resolution). Also owns the crash-recovery autosave slot (a fixed
             filename in the OS's per-user data directory, via `eframe::storage_dir`).
recent.rs    A small persisted "recently opened" list (separate for projects and
             audio), stored next to the autosave slot.
cdg.rs       Low-level CD+Graphics packet encoder - the 24-byte packet format, tile
             blocks, CLUT loads, and timing-by-packet-position. No knowledge of
             lyrics; just a byte-stream writer.
font.rs      Renders characters into CDG's 6x12 tile format (and 2x/3x/... scaled
             variants) from the `font8x8` bitmap font.
export.rs    The ".cdg renderer": lays out lines/words/countdown dots on the CDG
             canvas using cdg.rs + font.rs, driven by lyrics.rs's timing. Also owns
             the "paired audio" helper (copying the loaded audio next to a
             `.cdg`/`.lrc`/UltraStar file under a matching base filename). A line's
             backing vocal (if any) draws on its own row (BACKING_ROW) with its own
             word-wipe running concurrently with the host's - the two lines' word
             events are merged into one time-sorted sequence before being emitted,
             since CdgWriter::advance_to is monotonic-only-forward.
video.rs     The "video renderer": an independent RGB24 frame renderer (via
             `ab_glyph` for anti-aliased text) piped into an `ffmpeg` subprocess
             (this project's own bundled LGPL build, not a system install - see
             ffmpeg_path.rs), driven by the *same* lyrics.rs timing as export.rs.
             A backing vocal draws directly beneath the current line, smaller,
             with its own wipe, matching the same relationship the `.cdg` export
             uses.
ffmpeg_path.rs  Locates the bundled ffmpeg binary and the openh264 shared
             library its H.264 encoder needs (two different sourcing stories -
             see the "Bundled binaries and models" section below), the same
             bundled-resource-or-cache pattern model_assets.rs/onnxrt.rs use.
timeline.rs  Pure time<->pixel mapping, zoom/drag bounds, and drag-mode
             classification for the fine-tuning timeline - no egui dependency, so
             it's unit-tested directly; main.rs owns the actual widget/painting/
             interaction glue built on top of it.
vocals.rs    Runs the UVR-MDX-NET-Inst_HQ_3 separation model natively (via `ort`/
             ONNX Runtime - see mdx.rs/stft.rs) to produce instrumental and vocals
             stems from the loaded audio, for the "Instrumental audio"/"Vocals
             audio" export checkboxes and the video export's "Remove vocals"
             checkbox. Nothing to install separately - the model ships bundled
             with the app (see model_assets.rs).
mdx.rs       The MDX-Net separation algorithm itself: chunked overlap-add,
             STFT->model->ISTFT per chunk, the vocals-by-subtraction formula - a
             bit-faithful port of `audio-separator`'s reference implementation,
             not a from-scratch reimplementation of "an" MDX-Net.
stft.rs      Short-time Fourier transform matching PyTorch's `torch.stft`/
             `torch.istft` conventions exactly (reflect-padding, windowed
             overlap-add with NOLA normalization) - mdx.rs's building block.
model_assets.rs  Locates a bundled ONNX model file next to the installed app
             (falling back to a per-user cache directory, downloading into it if
             needed - a dev-build convenience for the separation model, but
             auto-align's language models always use this path, bundled release
             or not - see align.rs).
onnxrt.rs    Locates and loads the ONNX Runtime shared library itself (not a
             model file) that both vocals.rs/mdx.rs and align.rs/ctc.rs need,
             via `ort`'s `load-dynamic` feature - the same bundled-or-cached
             resolution model_assets.rs uses for model files, generalized to a
             library instead. Exists specifically because ONNX Runtime doesn't
             publish a prebuilt binary for macOS x86_64 at all (see the release
             workflow) - every platform goes through this one runtime-loading
             path rather than 3 platforms doing build-time linking
             (`download-binaries`) and macOS x86_64 alone doing something
             different.
align.rs     Runs a wav2vec2-CTC speech model natively (via `ort`, same as
             vocals.rs) once per already-timed line, restricted to that line's own
             tapped window, to fill in real word-level timing via forced alignment
             - "🪄 Auto-align words". The model isn't bundled (each language is
             ~1.2GB) - downloaded/cached via model_assets.rs on first use instead.
ctc.rs       The CTC forced-alignment trellis algorithm and vocab/tokenization
             logic behind align.rs - kept separate so it's unit-tested with
             synthetic emission matrices, no ONNX model needed.
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
   all rather than rejecting it after the fact. Correcting a value this way also
   triggers `KaraokeApp::audition_seek`, rewinding playback a couple seconds before
   the edited point for immediate audible feedback.

   `word_overrides`/`word_end_overrides` (the *word*-level equivalent) fill in
   similarly - clicking a word in the fine-tune panel, dragging a word bubble on the
   timeline, or (new) `align.rs`'s forced alignment, which sets every word in an
   already-timed line at once from real audio analysis instead of one click/drag per
   word. All of these write the exact same two fields, so which route produced a
   given word's timing is not itself tracked anywhere.
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

## Project files, autosave, and undo/redo

`project::ProjectFile` is a serde-serializable snapshot of everything needed to
resume a session: `lyrics_raw`, `lines: Vec<LyricLine>`, `title`/`artist`, colors
(as plain `RgbColor`, not `egui::Color32` - `project.rs` doesn't depend on egui),
video resolution, and the loaded audio's path. `KaraokeApp::to_project_file`/
`apply_project_file` are the only two places that convert between this and live
app state, so "Save Project", "Load Project…", crash recovery, and the periodic
autosave all funnel through the same two functions rather than each having their
own serialization logic. The crash-recovery autosave (`project::write_autosave`/
`read_autosave`/`clear_autosave`) reuses the exact same `ProjectFile`, written to a
fixed filename in the OS's per-user data directory (`eframe::storage_dir`) instead
of a user-chosen path - `main.rs` compares the live state against
`last_saved_snapshot` (`has_unsaved_changes`) to decide whether that autosave
should survive a clean exit.

Undo/redo (`KaraokeApp::undo_stack`/`redo_stack`, both `VecDeque<UndoSnapshot>`)
takes a different approach from typical command-pattern undo: rather than every
tap/drag/nudge/parse call site explicitly pushing an undo step, `track_undo_history`
runs once per frame (at the end of `update()`) and diffs the current
`UndoSnapshot` (lyrics text, lines, title, artist - a narrower subset than
`ProjectFile`, deliberately excluding colors/resolution) against what was
observed last frame. A continuous gesture (a timeline drag, or any focused text
field) is tracked via a `gesture_active` flag so a whole drag or typing session
collapses into one undo step instead of one per frame/keystroke, only finalized
once the gesture ends. This means no individual mutation site in `main.rs` needs
to know undo/redo exists at all - a new way to edit `self.lines` gets undo support
for free, as long as it doesn't need special coalescing behavior of its own.
`apply_project_file` clears both stacks, since undoing into a *different* loaded
project's content would be nonsensical.

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

The GUI runs on the main thread (`eframe`'s event loop). Every long-running
operation follows the same shape: a background thread reports progress through an
`Arc<AtomicU32>` (0..=1000, tenths of a percent) and its final result through an
`Arc<Mutex<Option<T>>>`, both polled from `update()` each frame by a `poll_*`
method that applies the result to `self` once it's ready and returns `true` while
still in progress (so the caller knows to keep calling `ctx.request_repaint()`).

- **Combined export** (`KaraokeApp::start_combined_export` /
  `CombinedExportHandle` / `poll_combined_export`) is *one* background thread
  covering any mix of `.cdg`, `.lrc`, UltraStar `.txt`, `.mp4`, and instrumental
  audio - not one thread per output. It reports a single overall progress
  fraction across equal-weighted phases (one per selected output), collecting a
  `Vec<ExportOutcome>` so each output succeeds or fails independently (one output
  failing doesn't stop the others). When "Remove vocals" is checked for a video
  export, separation runs *inside* this same thread first (into a throwaway temp
  WAV) before `video::render_video` is called with that file instead of the
  original audio; if both instrumental export and "Remove vocals" are requested
  together, separation runs once and both outputs reuse it
  (`shared_instrumental`). Audio pairing (copying the loaded audio next to
  whichever of `.cdg`/`.lrc`/UltraStar actually got written) also happens inside
  this thread, once, regardless of how many of those three were selected
  together.
- **Waveform extraction** (`start_waveform_job` / `WaveformJob` /
  `poll_waveform_job`) decodes the loaded audio into peaks on its own background
  thread whenever a new file is loaded (manually, via drag-and-drop, or via
  project load/recovery).
- **Auto-align** (`start_word_alignment` / `AlignJob` / `poll_align_job`) loads
  one `align::Aligner` (decodes the audio and loads the selected language's
  model once) and calls `align_line` per already-timed multi-word line,
  sequentially, on a single background thread, updating progress after each
  line. A whole-job-level failure (model failed to load/download) short-circuits
  before attempting any line; a per-line failure doesn't stop the rest.

Audio playback itself runs on `rodio`'s own output thread; `AudioPlayer` only
tracks a wall-clock position (`PlaybackClock`) on the main thread, deliberately
decoupled from the audio device so it can be unit-tested without real hardware.

## Bundled binaries and models (no external subprocess dependencies left)

Every feature that used to require a separate command-line install now runs
against something this project builds or fetches itself:

- **`ffmpeg`** (`video.rs`/`vocals.rs`, located via `ffmpeg_path.rs`) - required
  for `.mp4` video export (frames piped to it over stdin as raw RGB24, muxed
  with the loaded audio file) and for a non-`.wav` stem (`.mp3`) in vocal
  removal. No longer a system-installed dependency in a packaged build: this
  project compiles its own `ffmpeg` from source (`scripts/build-ffmpeg.sh`,
  run by the release workflow), `--disable-gpl` (LGPL v2.1-or-later only, no
  x264/x265), with H.264 via [openh264](https://github.com/cisco/openh264)
  (BSD) and MP3 via [LAME](https://lame.sourceforge.io) (LGPL; MP3's patents
  expired in 2017, so no separate patent question there, unlike H.264). A
  `cargo run` dev build without one bundled falls back to a system-installed
  `ffmpeg` on PATH - the only one of this project's bundled binaries that
  falls back to a system copy rather than downloading/erroring, since a
  system `ffmpeg` almost certainly already has its own H.264 encoder.
- **`libopenh264`** (also via `ffmpeg_path.rs`) - the shared library this
  app's `ffmpeg` build loads for H.264 encoding, and deliberately *not* a
  copy this project compiles itself: Cisco's patent-royalty coverage for
  OpenH264 (the reason it avoids H.264's usual per-encoder patent licensing
  cost) applies only to *Cisco's own* separately-distributed binary -
  confirmed directly against openh264's own `BINARY_LICENSE.txt`, not
  assumed. So `ffmpeg` links against openh264's headers/ABI at build time
  (a copy `scripts/build-ffmpeg.sh` compiles and discards, never
  distributed), but the actual bundled/downloaded runtime library is
  Cisco's official binary for that exact version - the same approach
  Firefox/Chromium use for the same reason. Getting the two builds' ABI to
  actually agree took real, verified work: openh264's own build bakes a
  *versioned* library name into what it produces (e.g. `libopenh264.so.8`,
  or an absolute `/build/path/libopenh264.8.dylib` on macOS) which won't
  match a generically-named downloaded file, so `scripts/build-ffmpeg.sh`
  overrides that to a simple, fixed name (verified on Linux with `readelf`
  and a full build+link+run+encode test against Cisco's real binary
  swapped in; macOS uses `install_name_tool` to the same end, unverified
  outside this project's own CI - see the script's comments).
- **Vocal removal** (`vocals.rs`/`mdx.rs`) and **auto-align**
  (`align.rs`/`ctc.rs`) used to shell out to the `audio-separator` Python CLI
  and `aeneas` (which itself needed eSpeak/eSpeak-NG and `ffmpeg`)
  respectively. Both now run their model natively via `ort` (ONNX Runtime
  bindings) - no subprocess, no separate install. The difference between
  them is bundling: vocal removal's ~65MB model ships inside every
  installer (see `model_assets.rs`); auto-align's models are ~1.2GB *each*,
  one per language, so only the language actually selected downloads, on
  first use, into a local cache.
- **The ONNX Runtime shared library itself** (`onnxrt.rs`) - a separate
  concern from either ML model file, needed by both of the features above.
  Loaded via `ort`'s `load-dynamic` feature rather than linked in at build
  time (`download-binaries`), specifically because ONNX Runtime publishes no
  prebuilt binary at all for one of this app's four release targets (macOS
  x86_64 - dropped upstream between versions 1.23 and 1.25). Rather than
  build that one target from source while the other three link a downloaded
  binary in normally, all four use the same runtime-loading path: a shared
  library bundled next to the installed app (built from source by the
  release workflow for macOS x86_64, downloaded from ONNX Runtime's own
  releases for the other three), or, in a dev build, the same per-user cache
  directory the ML model weights fall back to.

None of `ffmpeg`, `libopenh264`, either ONNX model, or the ONNX Runtime
library itself is a build-time dependency (`cargo build`/`cargo test` need
none of them) - all are only checked/fetched/loaded at runtime, right before
the feature that needs them actually runs, the same shape throughout: try
loading/locating what's needed, and if that fails, return a clear, actionable
error rather than a raw failure or a silent one later. This is also why
`cargo build`/`cargo test` make no network requests at all anymore - the
prior `ort` `download-binaries` approach did, on every build.

## Testing strategy

`cdg.rs`, `font.rs`, `lyrics.rs`, `formats.rs`, `export.rs`, `video.rs`, `timeline.rs`,
`project.rs`, `recent.rs`, `waveform.rs`, `align.rs`, `ctc.rs`, `stft.rs`, `mdx.rs`, and
`ffmpeg_path.rs` have no GUI dependency and are unit-tested directly (`cargo test`, no
display or audio device required). `audio.rs`'s `PlaybackClock` (the play/pause/resume/seek math) is
tested the same way, decoupled from the real `rodio`/`symphonia` calls it wraps.
`stft.rs`'s forward/inverse transform is verified by round-trip reconstruction tests
(`ISTFT(STFT(x)) ≈ x`, within floating-point tolerance) against both a sine wave and
pseudo-random noise, at MDX-Net's actual `n_fft`/`hop_length` - this is what gives
confidence the reflect-padding/windowing/NOLA-normalization convention actually matches
PyTorch's, not just that it compiles. `mdx.rs`'s chunk/pad/trim size constants are
checked directly against the values verified against the model's own data (see
`vocals.rs`'s module docs for how). `ctc.rs`'s forced-alignment trellis/backtrack is
verified against hand-computable synthetic emission matrices with an unambiguous correct
answer (not just "it runs") - this is what caught a real tie-breaking bug during
development (the backtrack initially preferred the wrong side of a near-tie, silently
shifting every word boundary later than it should be). `align.rs`'s bundled vocab files
are checked for internal consistency (every language parses, has a distinct blank and
word-delimiter token). Neither `ffmpeg`'s subprocess invocation nor either ML feature's
actual ONNX inference (which needs the real, multi-tens-of-MB to multi-GB model files,
not committed to the repo - see `model_assets.rs`) is exercised in the default test
suite - all are meant to be verified manually (`align.rs` and `vocals.rs` each have an
`#[ignore]`d real-model integration test for this, `real_model_smoke_test`, and
`ffmpeg_path.rs` has an equivalent one against a real bundled/cached ffmpeg,
`command_produces_a_working_ffmpeg` - see each test's own doc comment for how to run it).
`project.rs`/`recent.rs`'s tests use their own throwaway app id/temp directories so they
never touch a real run's actual autosave or recent-files slot. `waveform.rs`'s actual
audio decoding isn't exercised either (same reasoning - it needs a real file), just the
pure peak-lookup math. `main.rs` (GUI wiring, including the timeline widget's
painting/interaction code and the undo/redo frame-diffing built on top of the modules
above) has no automated tests - it's a thin layer that calls into the tested modules
above, verified manually by running the app.
