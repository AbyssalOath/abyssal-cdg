# Learn the Code

A guided tour of how Abyssal CDG Creator's source code actually works, for
anyone who wants to read it, fix something, or add a feature with
confidence. It's written for someone who can program but isn't necessarily
a Rust, DSP, or karaoke-format expert - jargon is defined the first time it
comes up (see the Glossary too), and every claim here is checked against
the real source rather than guessed.

This isn't a replacement for [ARCHITECTURE.md](../ARCHITECTURE.md) (the
concise reference) or [FEATURES.md](../FEATURES.md) (the user-facing
feature list) - it's the longer-form walkthrough that explains *how* the
code gets there, file by file, with pointers to exact functions and structs
you can jump to in your editor. This same content is also built into the
app itself as a "Learn the Code" section of the in-app Codex (Help menu)
- one source file, rendered in both places, so it can't drift between the
two the way a hand-duplicated copy would.

This guide is a living document: it's built incrementally alongside the
rest of the codebase, so some chapters below are still placeholders. See
[CONTRIBUTING.md](../CONTRIBUTING.md) for when/how to update it.

## Table of Contents

1. [Big Picture](#big-picture)
2. [Project Map](#project-map)
3. [Glossary](#glossary)
4. [App Core: `main.rs`](#app-core-mainrs)
5. [Data Model: `lyrics.rs`, `formats.rs`, `timeline.rs`, `project.rs`, `recent.rs`, `onboarding.rs`](#data-model-lyricsrs-formatsrs-timeliners-projectrs-recentrs-onboardingrs)
6. [Audio, DSP & ML](#audio-dsp--ml)
7. [Rendering & Output](#rendering--output)
8. [How the Modules Talk](#how-the-modules-talk)
9. [Deep Dives](#deep-dives)
10. [Rust Concepts Used Here](#rust-concepts-used-here)
11. [How-To Recipes](#how-to-recipes)
12. [Build, Test, CI & Release](#build-test-ci--release)
13. [Suggested Reading Order](#suggested-reading-order)
14. [Practice Exercises](#practice-exercises)
15. [Ideas for Future Improvements](#ideas-for-future-improvements)

## Big Picture

Abyssal CDG Creator turns an audio file and a set of lyrics into real
karaoke output: a legacy `.cdg` graphics file (the format actual karaoke
machines and MP3+G jukebox software read), a modern `.mp4` video, and/or
plain lyrics files (`.lrc`, UltraStar `.txt`) - any combination, from one
export dialog. The hard part isn't the file formats themselves; it's
knowing *when* each word is sung. Everything in this app exists to answer
that question as accurately as possible, in one data model that every
output format then reads from identically.

**The pipeline, in plain language:**

1. **Load audio.** You pick a file; `audio.rs` decodes/plays it and tracks
   playback position.
2. **Get lyrics in.** Either paste plain text, or import a file that
   already carries real timing (LRC, UltraStar, KOK) - `formats.rs` detects
   which format it is automatically.
3. **Time it.** Three ways to fill in when each line starts/ends, all
   converging on the same underlying fields so they stay interchangeable:
   tapping along with playback, dragging bubbles on the fine-tuning
   timeline, or typing a timecode directly.
4. **(Optional) Auto-align words.** Rather than guessing word-by-word
   timing from word count, `align.rs` runs a real speech-recognition model
   against the *already-timed* audio to find exactly where each word falls
   - and first isolates the vocals from the instrumental (`vocals.rs` +
   `mdx.rs`) so that speech model isn't fighting a wall of music.
5. **Resolve.** `lyrics::resolve_timing` turns the raw, user-edited line
   list into a clean, sequential `Vec<TimedLine>` - sorted, with each
   line's end chained to the next line's start.
6. **Render, three ways, from the same data.** `export.rs`/`cdg.rs` write
   the fixed 300x216 `.cdg` packet stream; `video.rs` renders arbitrary-
   resolution video frames and pipes them into a bundled `ffmpeg`;
   `formats.rs` writes the same timing back out as LRC/UltraStar text. The
   in-app live preview is a *fourth* reader of this same resolved timing,
   so what you see while editing matches what gets exported.

```
   Audio file                         Pasted / imported lyrics
       |                                        |
       v                                        v
  AudioPlayer                          Vec<LyricLine>
  (audio.rs)                       (lyrics.rs / formats.rs)
       |                                        |
       |      tap-to-time / timeline drag / typed timecode
       |                                        |
       |                     (optional) auto-align:
       |                     isolate vocals (vocals.rs + mdx.rs + stft.rs)
       |                       -> forced alignment (align.rs + ctc.rs)
       |                                        |
       |                                        v
       |                     lyrics::resolve_timing -> Vec<TimedLine>
       |                                        |
       +---------------------+------------------+------------------+
                              |                  |                  |
                              v                  v                  v
                       export.rs + cdg.rs     video.rs           formats.rs
                       (.cdg packet stream)   (frames -> ffmpeg)  (.lrc / UltraStar .txt)
                              |                  |                  |
                              v                  v                  v
                          .cdg file          .mp4 file          lyrics file
```

The fine-tuning timeline (owned by `main.rs`, math in `timeline.rs`) is
both a reader *and* a writer of the same `Vec<TimedLine>`/`Vec<LyricLine>`
data: dragging a bubble reads the resolved timing to position itself, then
writes back into the exact same `LyricLine` fields tapping and manual
entry use, so all three input methods stay perfectly interchangeable.
Later chapters cover each box in this diagram in depth.

## Project Map

What every top-level file and folder is for - a map to come back to
whenever you're not sure where something lives.

```
abyssal-cdg/
├── src/                  23 Rust modules - see the per-module chapters below
├── assets/
│   ├── fonts/            Bundled fonts: DejaVu Sans (video text), font8x8
│   │                     (.cdg's bitmap font comes from the font8x8 crate,
│   │                     not a file here), Creepster/Nosifer (selectable
│   │                     lyric fonts)
│   ├── icons/            App icon source images (.ico/.png)
│   ├── align_vocab/      Per-language vocab.json for forced alignment -
│   │                     see the Audio/DSP/ML chapter
│   ├── codex/            (if present) supporting assets for in-app docs
│   └── screenshots/      README/marketing screenshots
├── scripts/
│   └── build-ffmpeg.sh   Builds this project's own LGPL-only ffmpeg
│                         (+ openh264, LAME, dav1d, SVT-AV1) from source -
│                         run by the release workflow, not by `cargo build`
├── .github/workflows/
│   ├── ci.yml            Runs on every push/PR: fmt, clippy, test, build,
│   │                     cargo audit
│   └── release.yml       Builds and signs installers for all 4 platforms,
│                         fetches ffmpeg/openh264/ONNX Runtime/ML models
├── docs/
│   └── LEARN_THE_CODE.md This file
├── build.rs              Windows-only: embeds the app icon + version info
│                         into the compiled .exe's own PE resources
├── Cargo.toml            Dependencies, and `cargo packager` config (what
│                         gets bundled into each platform's installer)
├── Cargo.lock            Pinned dependency versions
├── README.md             Quick overview, install instructions, building
├── ARCHITECTURE.md       Concise module map, data flow, threading model
├── FEATURES.md           The full user-facing feature walkthrough
├── CONTRIBUTING.md       How to contribute, testing expectations, style
├── CHANGELOG.md          Notable changes by version
├── SECURITY.md           Threat model, how to report a vulnerability
├── BUILD_TROUBLESHOOTING.md  Common build failures and fixes
├── THIRD_PARTY_LICENSES.md  Every bundled/downloaded binary, model, and
│                         crate's license
├── DISCLAIMER.md         Legal disclaimer shown on first launch
└── LICENSE               This project's own license (AGPL-3.0)
```

`src/` itself, one line each (depth comes in later chapters):

| File | What it's for |
|---|---|
| `main.rs` | GUI shell (eframe/egui): owns all app state, wires every button/shortcut, hosts the live preview and fine-tuning timeline, undo/redo, drag-and-drop, crash recovery |
| `lyrics.rs` | The data model: `LyricLine` (raw/user-entered) and `TimedLine` (resolved), word-highlight timing, countdown-window math, timecode parsing |
| `formats.rs` | Import/export for LRC, UltraStar, and KOK lyric files, plus format auto-detection |
| `timeline.rs` | Pure time↔pixel mapping and drag math for the fine-tuning timeline - no egui dependency, unit-tested directly |
| `project.rs` | Save/load for `.abyzl` project files, plus the crash-recovery autosave slot |
| `recent.rs` | Persisted "recently opened" files list |
| `onboarding.rs` | The one-time first-run disclaimer flag |
| `audio.rs` | Audio decode/playback and the play/pause/seek state machine used for tap-to-time |
| `waveform.rs` | Decodes audio into coarse peaks, drawn behind the timeline as a visual aid |
| `ffmpeg_path.rs` | Locates the bundled `ffmpeg` binary and the `openh264` shared library it needs |
| `cdg.rs` | Low-level CD+Graphics packet encoder - no knowledge of lyrics, just a byte-stream writer |
| `font.rs` | Renders characters into CDG's 6x12 tile bitmap format |
| `export.rs` | The `.cdg` renderer: lays out lines/words/countdown dots on the CDG canvas |
| `video.rs` | The `.mp4` renderer: RGB24 frames piped into `ffmpeg`, anti-aliased text |
| `vocals.rs` | Runs MDX-Net vocal/instrumental separation natively via ONNX Runtime |
| `mdx.rs` | The MDX-Net separation algorithm itself (STFT → model → ISTFT, chunked) |
| `stft.rs` | Short-time Fourier transform matching PyTorch's exact convention |
| `align.rs` | Runs a wav2vec2-CTC speech model for forced word-level alignment |
| `ctc.rs` | The CTC forced-alignment trellis/tokenization math, no ONNX dependency |
| `model_assets.rs` | Locates/downloads/hash-verifies bundled ONNX model files |
| `onnxrt.rs` | Locates/loads the ONNX Runtime shared library itself |
| `fonts.rs` | System font enumeration/loading for the selectable lyric-text font |
| `codex.rs` | The in-app documentation browser you may be reading this in right now |

## Glossary

Terms the code and this guide use, defined once here rather than
re-explained every time. More are added as later chapters need them.

- **CD+G (CDG)** - "Compact Disc + Graphics," a legacy format that encodes
  a simple graphics display as data hidden in an audio CD's subchannel.
  This app's `.cdg` export writes this exact byte format: a fixed
  300x216-pixel, 16-color display, played back by real karaoke machines
  and MP3+G/karaoke jukebox software.
- **Tile** - CD+G's smallest drawable unit: a fixed 6-pixel-wide,
  12-pixel-tall block. Every piece of text or graphics in a `.cdg` file is
  built from tiles - see `cdg.rs`/`font.rs`.
- **Palette / CLUT** - CD+G's Color Look-Up Table: 16 color slots (4 bits
  per RGB channel each), loaded in two 8-color halves. Every pixel in a
  tile picks one of the 16 by index, not by its own RGB value.
- **MP3+G** - The convention of pairing a `.cdg` file with an audio file of
  the same base name in the same folder (CD+G itself carries no audio) -
  what karaoke jukebox software expects to find.
- **STFT (Short-Time Fourier Transform)** - Splits an audio signal into
  short overlapping windows and runs a Fourier transform on each one,
  producing a time-by-frequency picture of the sound (a spectrogram).
  `stft.rs` implements this to match PyTorch's exact convention, since the
  ML models here were trained against spectra produced that specific way.
- **MDX(-Net)** - A neural network architecture for music source
  separation (splitting a mix into stems like "vocals" and
  "instrumental"). This app runs a pretrained MDX-Net model natively via
  ONNX Runtime - see `mdx.rs`/`vocals.rs`.
- **Forced alignment** - Given audio *and* the exact text being spoken/sung
  (no guessing what was said), finding *when* in the audio each word
  occurs. This app uses a CTC-based forced-alignment model - see
  `align.rs`/`ctc.rs`.
- **CTC (Connectionist Temporal Classification)** - A way of training and
  decoding speech models that doesn't require frame-by-frame-labeled
  training data; its forced-alignment variant (used here, not CTC's more
  common "decode unknown speech" use) finds the highest-probability way to
  align a *known* target text against per-frame model output.
- **ONNX / ONNX Runtime** - ONNX is an open, portable format for trained
  ML models; ONNX Runtime is the engine that actually runs one. This app
  loads ONNX Runtime as a shared library at runtime (`onnxrt.rs`) and runs
  real pretrained models through it (`mdx.rs`, `align.rs`) - no Python, no
  separate install.
- **Hann window** - A smooth, bell-shaped weighting curve applied to each
  STFT frame before transforming it, which avoids the sharp artifacts a
  hard-edged (unweighted) frame would introduce.
- **Overlap-add / NOLA** - The standard way to reconstruct a continuous
  signal from overlapping windowed frames (the *inverse* STFT): each
  frame's samples are added into their overlapping position, then
  normalized by the overlap-added *squared* window (the NOLA, "nonzero
  overlap-add," formula) rather than divided by a simple count.
- **Vocab / blank token / word-delimiter token** - A CTC model's vocabulary
  maps each character it recognizes to a numeric ID, plus two special IDs:
  a "blank" (meaning "no new character emitted this frame," the mechanism
  that lets CTC align a long audio clip to a much shorter text) and a
  word-delimiter (how the model represents the pause between words in
  continuous speech).
- **Trellis** - The dynamic-programming grid `ctc.rs`'s forced-alignment
  algorithm builds: one axis is time (audio frames), the other is position
  in the target text, and each cell holds the best-probability path to
  that point - the standard structure behind "align this exact text to
  this exact audio."
- **"Sing end" / wipe fraction** - This app's own terms, not DSP jargon:
  `sing_end` is when a line's estimated/actual singing finishes (which can
  be well before its on-screen `end`, if there's a musical break after
  it); "wipe fraction" is how far the karaoke color-wipe has progressed
  across a line's text at a given moment, 0.0 to 1.0.
- **Countdown window** - The `(start, end)` time range during which this
  app's "get ready" countdown dots should be visible, computed by
  `lyrics::countdown_window` whenever a gap between lines is long enough
  to warrant one.

## App Core: `main.rs`

**Purpose.** `main.rs` is the GUI shell: it owns every piece of live
session state, draws the entire window every frame, wires every button/
shortcut/drag to the pure-logic modules that actually do the work, and
hosts the three pieces of UI with real complexity of their own (the live
preview, the fine-tuning timeline, and the timing table). Nothing in this
app calls *into* `main.rs` - it's the top of the dependency tree: it calls
almost every other module (`audio.rs`, `lyrics.rs`, `formats.rs`,
`project.rs`, `recent.rs`, `onboarding.rs`, `timeline.rs`, `export.rs`,
`video.rs`, `vocals.rs`, `align.rs`, `mdx.rs`, `fonts.rs`, `waveform.rs`,
`codex.rs`) and is itself only called by `fn main()` at the very bottom of
the file.

At 6,300+ lines it's by far the largest file in the project - and
structurally different from everything else in it. Every other module in
this app is pure logic with no GUI dependency and real unit tests (see
`ARCHITECTURE.md`'s "Testing strategy"). `main.rs` is the opposite: almost
entirely GUI wiring, verified by running the app rather than by
`cargo test`, with its only unit-tested pieces being a handful of free
functions deliberately pulled out specifically *because* they're pure
logic (`apply_row_selection_click`, `parse_artist_title_from_filename`,
`sanitize_export_base_name`, `timeline_cursor_icon` - see their own tests
at the bottom of the file).

### The `KaraokeApp` struct

One big struct holds *all* live session state - there's no separate
"model" object; `main.rs` owns the data and the widgets that edit it
together. Grouped by concern (the struct itself isn't literally ordered
this way, but it clusters similarly):

| Group | Fields (representative, not exhaustive) | Notes |
|---|---|---|
| Audio & project identity | `audio`, `lyrics_raw`, `lines`, `current_project_path`, `title`, `artist` | `lines: Vec<lyrics::LyricLine>` is the actual song data - everything else is either derived from it or metadata about it |
| Crash recovery / save state | `pending_recovery`, `last_autosave`, `last_saved_snapshot` | `last_saved_snapshot` is compared against a freshly-built `to_project_file()` every time `has_unsaved_changes()` is asked |
| Tapping | `next_untimed`, `tap_phase`, `word_tap_line`, `word_tap_mode`, `backing_vocal_edit` | drives the "Tap next line" button/Space key and the per-word fine-tune panel |
| Timing table UI | `start_edit`, `end_edit`, `selected_lines`, `selection_anchor`, `compact_timing_table` | `start_edit`/`end_edit` hold in-progress typed text separately from the committed value - see "Gotchas" below |
| Colors | `color_bg`, `color_male_unsung`, `color_male_highlight`, ... (one `egui::Color32` pair per `Singer` variant, plus background/preview/title/artist), `color_preset` | `color_preset` is cosmetic only - it remembers which preset button was last clicked, for the dropdown's own display, and doesn't stay "locked in" against manual edits |
| Video/background/font | `video_resolution`, `video_codec`, `show_credit_line`, `background`, `background_fit`, `background_dim`, `background_preview`, `selected_font_family`, `font_list`, `font_bytes`, `font_load_error`, `installed_egui_font` | `background_preview`'s inner `Option<TextureHandle>` being `None` specifically means "tried and failed," distinct from the outer `Option` being `None` ("nothing selected") |
| Export dialog | `export_cdg`, `export_lrc`, `export_ultrastar`, `export_video`, `export_instrumental`, `export_vocals`, `lrc_enhanced_words`, `export_folder`, `export_base_name`, `instrumental_format`, `combined_export` | session-only - reset to defaults by `apply_project_file`, *not* part of `project::ProjectFile` |
| Timeline widget | `timeline_view`, `timeline_drag`, `waveform`, `waveform_job` | `timeline_view: timeline::View` is the only timeline state that's pure/testable; drag interaction lives here |
| Auto-align | `align_language`, `align_vocal_model`, `align_job` | |
| Undo/redo | `undo_stack`, `redo_stack`, `undo_last_observed`, `undo_pending_baseline` | see "Undo/redo" below - this is a frame-diffing scheme, not a command pattern |
| Codex/onboarding | `codex_open`, `codex_selected`, `codex_cache`, `show_disclaimer_modal` | |

**Supporting types**, defined just above or around `KaraokeApp`:

- `WordTapMode` / `TapPhase` - which timestamp a click/Space press sets next.
- `PendingUnsavedAction` - `NewProject` or `LoadProject(PathBuf)`, held while `draw_unsaved_action_confirm_prompt` waits for the user to decide.
- `TimelineDragTarget` (`Line` or `Word(usize)`) and `TimelineDrag` (which line/word, plus a `timeline::DragSession`) - captured once at drag start and resolved fresh from the *original* pointer position every frame (not accumulated deltas - see Gotchas).
- `InstrumentalFormat`, `ColorPreset` - small UI-facing enums with a `label()`/lookup method each, the same shape `lyrics::Singer`/`video::Resolution` use.
- `ExportOutcome`, `CombinedExportHandle`, `UndoSnapshot`, `WaveformJob`, `FontListJob`, `AlignOutcome`/`AlignJobResult`/`AlignJob` - background-job plumbing, see "The background-job pattern" below.

### App lifecycle

- **`KaraokeApp::new()`** - builds every field's starting value, opens the
  default audio output device (`AudioPlayer::new()`), reads the
  crash-recovery autosave (`project::read_autosave`) into
  `pending_recovery`, loads the recent-files list, checks the onboarding
  flag, and kicks off the one-time background font enumeration
  (`start_font_list_job`). Called both by `main()` on real startup *and*
  by `start_new_project()` to reset everything at once (see Gotchas).
- **`update(&mut self, ctx, _frame)`** (the `eframe::App` trait's one
  required method) - runs once per frame. ~1,700 lines, almost entirely
  inline UI layout rather than factored into helper methods (see "Why
  `update` is so long" below). Its rough shape, top to bottom:
  1. **Early-exit modal states** - if `pending_recovery`/`show_quit_confirm`/
     `pending_unsaved_action` is set, draw *only* that prompt and `return`
     immediately - nothing else in the frame runs until it's resolved.
  2. **Housekeeping** - handle dropped files, fire the periodic autosave
     if due, poll every background job (`poll_combined_export`,
     `poll_waveform_job`, `poll_align_job`, `poll_font_list_job`), load/
     install the custom lyric font if needed.
  3. **Keyboard shortcuts** - Ctrl+S/Ctrl+Shift+S always active; Space/
     arrow keys/Ctrl+Z/Ctrl+Shift+Z/M/F/D/S gated behind `!typing` (no
     text field focused) and (for undo/redo) no timeline drag in progress.
  4. **Top panel** - title, transport controls, seek bar, title/artist
     fields.
  5. `draw_export_dialog`, `draw_codex`, `draw_disclaimer_modal` (each a
     no-op unless its own `show_*`/`*_open` flag is set).
  6. **Bottom panel** - export/auto-align buttons and progress bars.
  7. **Bottom timeline panel** - calls `draw_timeline`.
  8. **Left side panel** ("lyrics_panel") - the paste box and tap-along
     controls.
  9. **Right side panel** ("colors_preview_panel") - live preview
     (`draw_preview`), color grid, background picker, font picker, timing
     settings sliders.
  10. **Central panel** - the per-word fine-tune strip (if a line is
      selected) and the timing table (two side-by-side `egui::Grid`s).
  11. `self.track_undo_history(gesture_active)` - always called last.
- **`on_exit`** - if a recovery prompt was showing unanswered, leave the
  autosave alone; otherwise clear it only if there are no unsaved changes.
- **`main()`** - sets a 1350×800 default window size and hands
  `KaraokeApp::new()` to `eframe::run_native`. `APP_ID` ("Abyssal CDG
  Creator") is the one constant shared between the window title and every
  `eframe::storage_dir`/autosave/recent-files lookup.

### Project files, undo/redo, and the unsaved-changes guard

- **`to_project_file()`** / **`apply_project_file()`** are the *only* two
  places that convert between live `KaraokeApp` state and
  `project::ProjectFile` - "Save Project," "Load Project…," crash
  recovery, and the periodic autosave all funnel through these two, so
  there's exactly one place to fix if a field is ever missing from a
  save/load round trip. `apply_project_file` also resets every bit of
  *session-only* state that doesn't belong to the project itself (timeline
  zoom, in-progress text edits, the export dialog's checkboxes, undo
  history) - see Gotchas for why this matters.
- **`has_unsaved_changes()`** - `true` if there's any content at all
  (lines or loaded audio) *and* a fresh `to_project_file()` doesn't equal
  `last_saved_snapshot`. Drives both the window-close interception and
  whether `on_exit` keeps the crash-recovery autosave around.
- **Undo/redo** (`undo_stack`/`redo_stack`, both `VecDeque<UndoSnapshot>`,
  capped at `UNDO_HISTORY_LIMIT` = 100) is a **frame-diffing** scheme, not
  a command pattern: `track_undo_history(gesture_active)` runs once per
  frame and compares a fresh `undo_snapshot()` (just `lyrics_raw`,
  `lines`, `title`, `artist` - deliberately narrower than a full
  `ProjectFile`) against what was observed last frame. The moment they
  differ, it remembers the *previous* value as a pending baseline; once
  `gesture_active` goes false (no timeline drag, no focused text field),
  that baseline is pushed onto `undo_stack` as one step - so a multi-frame
  drag or a whole typing session collapses into a single undo, and no
  individual tap/drag/nudge call site needs to know undo exists at all.
  `apply_undo_snapshot`/`apply_project_file` both reset
  `undo_last_observed` afterward so the restoration itself is never
  mistaken for a new edit.
- **Save/load/new-project flow**: `request_project_load`/
  `request_new_project` check `has_unsaved_changes()` first - if there's
  something at risk, the action is stashed in `pending_unsaved_action` and
  `draw_unsaved_action_confirm_prompt` takes over the whole frame (see
  lifecycle step 1) instead of running immediately. `save_project` writes
  straight back to `current_project_path` if there is one, else falls
  back to `save_project_as_dialog`. `start_new_project` is
  `project::clear_autosave(APP_ID)` followed by `*self = KaraokeApp::new()`
  - see Gotchas for why clearing the autosave first matters.

### Lyrics in: paste, import, drag-and-drop

- **`parse_lyrics()`** - runs `lyrics::merge_reparsed_lyrics` against the
  paste box's current text, which is what lets editing a typo *not* wipe
  existing timing (see the Data Model chapter for how the merge itself
  works).
- **`load_lyrics_file(path)`** - reads the file, calls
  `formats::detect_format` then the matching importer
  (`import_lrc`/`import_ultrastar`/`import_kok`/`import_plain_text`), and
  rebuilds `lyrics_raw` from the result so the paste box stays consistent
  with whatever just got imported.
- **`handle_dropped_files(ctx)`** - routes each dropped file by extension
  to `load_audio`, `load_lyrics_file`, or `request_project_load` (`.abyzl`)
  - multiple files dropped together are each routed independently.

### Tapping, keyboard shortcuts, and the per-word panel

- **`tap_next()`** - the Space-key/button handler while any line still
  needs timing: sets `next_untimed`'s start or end (alternating via
  `tap_phase`) from the live playback position, validated through
  `lyrics::check_start_change`/`check_end_change` first. Once every line
  has both, Space switches to driving playback instead
  (`toggle_play_pause`) - checked in `update`, not in `tap_next` itself.
- **`recompute_next_untimed()`** - finds the first line still missing a
  start, called after *any* operation that could change which lines are
  timed (parsing, importing, undo/redo, loading a project).
- **`audition_seek(near_time)`** - rewinds playback
  `AUDITION_REWIND_SECS` (2s) before a just-corrected timestamp, called by
  every "I just set/dragged/nudged a time" path so you can immediately
  hear whether it landed right.
- **`assign_singer_to_current_line(singer)`** - the M/F/D/S shortcut
  handler.
- The per-word fine-tune strip (inline in `update`'s central panel, not
  its own function) shows one clickable button per word of
  `self.lines[word_tap_line]`, colored green/blue/teal for manual
  start/end/both - clicking calls `tap_word_start`/`tap_word_end`.

### The timing table

Also inline in `update`'s central panel (there is no `draw_timing_table`
function - see "Why `update` is so long"), built from **two
side-by-side `egui::Grid`s sharing one vertical `ScrollArea`**:
`lines_grid_sticky` (just the row-selection checkboxes, in their own
fixed-width column) and `lines_grid_rest` (Start/Lyric/End/Singer/
Countdown/action columns, inside its *own* horizontal `ScrollArea`) - so
the checkbox column stays visible no matter how far right you've scrolled
to reach the later columns, while both grids' rows still scroll
vertically in lockstep. Clicking a checkbox records a
`(line_idx, shift, cmd)` triple for `apply_row_selection_click` (one of
the few pure, unit-tested free functions in this file) to resolve
afterward.

### The fine-tuning timeline widget

`draw_timeline` is hand-rolled immediate-mode drawing (`ui.painter_at`,
manual rects) on top of `timeline.rs`'s pure math - it does *not* use any
egui layout widget for the bubbles themselves. Per frame: compute
`resolved_with_indices()`, draw the time ruler and waveform backdrop,
then one rect per timed line (clipped if off-screen), each wired to
`ui.interact` for click/drag with its own stable `Id` (`("timeline_bubble",
orig_idx)`). A drag start captures a `TimelineDrag` (line/word index +
`timeline::DragSession`); every subsequent frame of that same drag calls
`DragSession::resolve` against the pointer's *current* position relative
to where the drag *started*, never accumulating per-frame deltas (see
Gotchas for why). Releasing the drag writes the resolved value(s) back
into the exact same `LyricLine` fields tapping and the timing table use,
then calls `audition_seek`.

### Colors, background, and fonts

- **`palette()`** / **`video_palette()`** convert the live `egui::Color32`
  fields into `export::Palette` (CDG's 4-bit-per-channel colors, via
  `cdg_from_color32`) and `video::VideoPalette` (full 8-bit, via a local
  closure) respectively - two different color representations for the two
  renderers, both derived from the *same* `egui::Color32` source fields
  every frame, never stored twice.
- **`apply_color_preset(preset)`** - sets all 13 colors at once from
  `ColorPreset::palette()`, and only *also* sets a font if
  `preset.default_font()` returns one (only "Abyssal" does, defaulting to
  "Nosifer") - a one-time action, not a persistent mode; manually tweaking
  one color afterward doesn't un-apply or re-lock anything.
- **`ensure_background_preview`** decodes/extracts a background image's
  or video's poster frame into a cached `egui::TextureHandle`, keyed
  against `(Background, BackgroundFit, Color32)` so it's rebuilt only when
  one of those three actually changes - not every frame.
- **`ensure_font_loaded`** / **`ensure_egui_font_installed`** are a
  two-step pipeline: the first loads raw font bytes from disk/bundled
  fonts (`fonts::load_family_bytes`) into `font_bytes`, cached until
  `selected_font_family` changes; the second registers those bytes with
  egui's own font system (`ctx.set_fonts`) only when the *installed* name
  actually differs from the target - `set_fonts` rebuilds the whole font
  atlas, so this guards against doing that every frame.

### The background-job pattern

Every slow operation (vocal separation + combined export, waveform
decoding, system-font enumeration, auto-align) follows the *same* shape,
first described in `ARCHITECTURE.md`'s "Threading" section and worth
seeing concretely once:

1. A `start_*` method spawns `std::thread::spawn`, moving owned copies of
   everything the thread needs (never a `&self` reference - the thread
   outlives the frame that started it).
2. Progress reports through an `Arc<AtomicU32>` (0..=1000, tenths of a
   percent) the thread stores into and the main thread reads every frame.
3. The final result reports through an `Arc<Mutex<Option<T>>>` - `None`
   while running, `Some` exactly once when done.
4. A `poll_*` method, called every frame from `update`, locks the mutex,
   `.take()`s the result if present (applying it to `self` and clearing
   the job), and returns `true` while still in progress so the caller
   knows to `ctx.request_repaint()` (otherwise the UI would only update on
   the next *user-driven* repaint, not when a background thread finishes).

`start_combined_export`/`poll_combined_export` (covering any mix of
`.cdg`/`.lrc`/UltraStar/`.mp4`/instrumental/vocals output) and
`start_word_alignment`/`poll_align_job` (vocal isolation, then one
`align::Aligner::align_line` call per already-timed multi-word line) are
the two most involved examples - both report one combined progress value
across multiple internal phases, and both let individual outputs/lines
fail independently without aborting the rest of the run.

### Non-obvious design decisions, invariants, and gotchas

- **Why `update` is so long.** Unlike the small, well-factored `draw_*`
  helpers for self-contained *windows* (`draw_export_dialog`,
  `draw_codex`, the three confirmation prompts), the timing table, the
  color grid, the background/font panels, and the per-word strip are all
  written directly inline inside `update`'s panel closures rather than
  pulled into their own methods - there's no `draw_timing_table` or
  `draw_color_panel`. This isn't an oversight to "fix": each of those
  pieces reads and writes many `self` fields interleaved with the layout
  code, and egui's closure-heavy API makes extracting a sub-piece require
  either passing a long list of `&mut` fields individually or restructuring
  around a sub-struct - a real refactor, not a free one. Worth knowing so
  you don't go looking for a method that doesn't exist.
- **Deferred mutation inside the timing table's loop.** The per-row loop
  can't call `self.tap_word_start(...)` (or similar) *while* also reading
  `self.lines[i]` for that same row - that would need two overlapping
  borrows of `self`. Instead, the loop collects what *would* happen into
  local `Option`s (`retap_idx`, `commit_start`, `selection_click`, ...)
  and applies them in a second pass after the loop ends. This pattern
  (collect intents during layout, apply them afterward) recurs anywhere
  in `main.rs` that needs to mutate `self` from inside a loop over
  `self`'s own data.
- **Timeline drags resolve from the original pointer position, every
  frame** - never by accumulating per-frame deltas. `TimelineDrag`
  captures the pointer position and original start/end *once*, at drag
  start; `DragSession::resolve(current_pointer_x, ...)` is then called
  fresh each frame with that same origin. Accumulating deltas frame-to-
  frame would drift under egui's own pixel rounding over a long drag -
  resolving from a fixed origin can't drift, by construction.
- **`drag_started()`, not `interact_pointer_pos()`, when starting a
  timeline drag.** egui only recognizes a drag once the pointer has moved
  past its own click-vs-drag threshold (6px as of egui 0.28) - by the
  time `drag_started()` fires, `interact_pointer_pos()` already reflects
  a position shifted a few pixels from where the drag *actually* began.
  `draw_timeline` captures the position at `drag_started()` time via a
  different path to avoid baking in that offset (see the code's own
  comment at that exact call site for the precise mechanism).
- **Three parallel color representations, one source of truth.**
  `egui::Color32` (live UI state) is the only one actually stored on
  `self`; `cdg::CdgColor` (4-bit, for `.cdg`) and `video::Rgb8` (8-bit,
  for `.mp4`) are derived fresh every time `palette()`/`video_palette()`
  are called, and `project::RgbColor` (a plain 8-bit struct with no
  `egui` dependency, for JSON serialization) is derived only at
  save/load time via `rgb_color_from_color32`/`color32_from_rgb_color`.
  Never written back the other way except through those conversions.
- **`start_new_project` clears the autosave *before* resetting state, not
  after.** `*self = KaraokeApp::new()` itself re-reads whatever's in the
  autosave slot into the fresh `pending_recovery` field - if the clear
  happened second (or not at all), a "New Project" click right after an
  explicit save would immediately show a false "Recover previous
  session?" prompt, because the periodic 30-second autosave (which "Save
  Project" never touches - it writes a completely different file) would
  still be sitting there. This was a real, reported-and-fixed bug in this
  app's own history, not a hypothetical - see `start_new_project`'s own
  doc comment for the full explanation.
- **`resolved_with_indices` duplicates `lyrics::resolve_timing_with_settings`'s
  sort-and-chain logic**, rather than calling it, specifically to also
  track each result's *original* index into `self.lines` (which isn't
  itself sorted by start time) - needed by the timeline and the
  auto-align request builder, neither of which can work with a sorted-only
  `Vec<TimedLine>`. `resolved()` (used by export and the preview, which
  don't need the index mapping) calls the real `lyrics.rs` function
  directly. Two implementations of the same sort/chain math exist for this
  reason - worth knowing if the chaining rule in `lyrics.rs` ever changes,
  since this one would need updating too.
- **Session-only export-dialog state is deliberately *not* part of
  `project::ProjectFile`.** Which outputs are checked, the chosen folder/
  base name, LRC2-vs-LRC1 - none of it round-trips through a save/load.
  `apply_project_file` resets it to fresh defaults on every load
  specifically so a newly opened project never silently inherits the
  *previous* project's export choices (a real bug this project fixed
  once - see `apply_project_file`'s own doc comment).

## Data Model: `lyrics.rs`, `formats.rs`, `timeline.rs`, `project.rs`, `recent.rs`, `onboarding.rs`

Unlike `main.rs`, every file in this chapter is pure logic with no GUI
dependency, and every one is covered by real unit tests (`cargo test`, no
display or audio device needed - see `ARCHITECTURE.md`'s "Testing
strategy"). They're presented here in dependency order rather than the
order they were originally listed in: `lyrics.rs` is the foundation
everything else in this chapter builds on, `formats.rs` and `timeline.rs`
each consume it from a different angle (file import/export, and UI math,
respectively), and `project.rs`/`recent.rs`/`onboarding.rs` are the three
small persistence utilities that save/restore session state to disk.

### `lyrics.rs` - the data model

**Purpose.** This is the one place that decides *what a timed lyric line
actually is* and *when each word lights up* - every renderer (`.cdg`,
`.mp4`, the live preview) and every timing-input method (tapping, the
timeline, typed timecodes, auto-align) reads or writes through this
module's types and functions, never duplicating the math itself. It has
**zero dependencies on any other module in this crate** - only `std` and
`serde` - which is exactly why it can be the foundation everything else
is built on.

**Key types:**

| Type | What it holds |
|---|---|
| `Singer` | `Default`/`Male`/`Female`/`Duet`/`Screaming` - which voice sings a line. `Default` has its own customizable color pair (not forced to look like `Male`), but starts out rendering identically to it. |
| `CountdownMode` | `Auto`/`Force`/`Suppress` - per-line override for whether the "get ready" countdown shows before this line. |
| `TimingSettings` | User-configurable overrides for `SECONDS_PER_WORD`, `MIN_SING_DURATION`, `COUNTDOWN_GAP_THRESHOLD` - `Default` is exactly those three constants, so a project that never opens the Timing panel is byte-identical to before this setting existed. |
| `LyricLine` | The **raw, user-edited** form: `text`, optional `start`, `singer`, per-word `word_overrides`/`word_end_overrides` (both `Vec<Option<f64>>`, one slot per word), optional `sing_end_override`, `starts_new_block`, optional `backing_vocal`, `countdown_mode`, optional `custom_singer_name`. |
| `BackingVocal` | A second vocalist's line, embedded on its host `LyricLine` rather than a top-level entry of its own (see Gotchas) - same `word_overrides`/`word_end_overrides` shape as its host, scoped to its own text. |
| `TimedLine` | The **resolved, always-has-a-window** form `LyricLine` turns into for export/preview: adds `start`, `end` (next line's start, or the song's end), and `sing_end` (`<= end`; the word-wipe's own window; any gap after it is a musical break). |
| `TimedBackingVocal` | `BackingVocal`'s resolved counterpart, same relationship. |
| `TimedWord` | One word's derived `highlight_at`/`held_until`, from [`word_timings`]. |
| `ReparseReport` | Tallies what re-parsing changed (unchanged/reworded-same-word-count/reworded-word-count-changed/added/removed) - see `merge_reparsed_lyrics`. |

**Key functions**, grouped by what they do:

- **Parsing text in**: `parse_pasted_lyrics(raw) -> Vec<LyricLine>` - one
  line per non-empty, non-section-marker (`[Verse 1]`-style) input line;
  blank lines/section markers are dropped but their *position* is kept by
  marking the following line `starts_new_block`. `merge_reparsed_lyrics(old,
  raw) -> (Vec<LyricLine>, ReparseReport)` is what "Parse lyrics" actually
  calls - see Gotchas for why it exists instead of just calling
  `parse_pasted_lyrics` directly.
- **Resolving timing**: `resolve_timing_with_settings(lines, total_duration,
  settings) -> Vec<TimedLine>` sorts by `start`, chains each line's `end`
  to the next line's `start` (or the audio's duration/`start + 4.0` for the
  last line), and derives `sing_end` per line. Takes every line with a
  `start` set; a line with none is silently excluded from the result (not
  an error - it just isn't "timed" yet).
- **Word timing**: `word_timings(line) -> Vec<TimedWord>` spreads words
  across `[start, sing_end)` proportional to character count, unless
  `word_overrides`/`word_end_overrides` pin a specific word - see Gotchas
  for the exact start/end resolution order. `current_line_wipe_fraction(line,
  t) -> f32` turns that into the single continuous 0.0-1.0 value the
  color-wipe actually animates.
- **Countdown/fade math**: `countdown_window`/`countdown_window_between`
  decide if/when the "get ready" dots should show; `hide_upcoming_lines`
  decides if not-yet-started lines in the same block should be hidden
  during a real break; `sung_line_fade_alpha`/`reveal_fade_alpha`/
  `slot_fade_alpha`/`rolling_preview_lines` are the video/live-preview-only
  fade/roll-in curves `.cdg` doesn't use (its fixed 16-color canvas has no
  reasonable way to do a smooth fade).
- **Validation**: `check_start_change`/`check_end_change` (a line's own
  timing) and `check_backing_vocal_start`/`check_backing_vocal_end` (bounded
  to the host's window) - all return `Result<(), String>` without applying
  anything, so a caller (tap, drag, typed timecode) can reject a change
  *before* committing it.
- **Timecode text**: `format_timecode`/`parse_timecode` - `MM:SS.CC`,
  round-trip-safe, `parse_timecode` returns `None` (not `0.0`) for anything
  that doesn't look like a time.
- **Blocks**: `group_into_blocks(timed_lines) -> Vec<Vec<usize>>` - splits
  wherever `starts_new_block` is set, capped at `MAX_BLOCK_LINES` (5) lines
  per block, shared by `video.rs` and the live preview so both group
  identically.

**Callers/callees.** Called by `main.rs` (extensively), `export.rs`,
`video.rs`, and `formats.rs` (for `LyricLine`/`TimedLine`/`Singer`). Calls
nothing else in this crate.

**Non-obvious design decisions and gotchas:**

- **Word-level timing is derived, not stored**, unless a word has been
  explicitly tapped/dragged/aligned. This is *why* tapping only needs two
  presses per line (start, end) instead of one per word - the character-
  weighted spread in `resolve_word_timings` is what fills in everything
  between, and it's just an estimate until fine-tuned.
- **A word's automatic *end* is "the next word's resolved *start*"**, not
  a fraction of the line's own duration - `resolve_word_timings` resolves
  every word's start first, then derives defaults from that same list, so
  the wipe runs continuously with no gaps unless a manual
  `word_end_overrides` entry opens one deliberately.
- **`BackingVocal` is embedded on its host line, not a second top-level
  line.** Two reasons, both load-bearing: it sidesteps needing a stable
  cross-line reference that would have to survive re-parsing (a host
  match via `merge_reparsed_lyrics` brings its backing vocal along for
  free), and it keeps the main `Vec<TimedLine>`'s sequential, non-
  overlapping shape unchanged - a backing vocal is purely an *extra* thing
  drawn during its host's own window, bounded to `[host_start, host_end)`,
  never an independent timeline entry.
- **Validation only ever checks explicit neighboring times, never
  automatic estimates.** `check_start_change`/`check_end_change`
  deliberately look at a neighbor's `sing_end_override` (if any), not its
  *derived* `sing_end` - the estimate shifts as timing fills in around it,
  so treating it as a hard wall would reject perfectly reasonable taps
  just because a neighboring line hasn't been fine-tuned yet.
- **`merge_reparsed_lyrics` exists because re-parsing used to silently
  wipe every line's timing** on any edit (even fixing a single typo) -
  a real, serious problem on an already-tapped/tuned song. It matches
  lines by a longest-common-subsequence pass first (`lcs_matches` - exact
  text match, in order, O(n·m)), which is what keeps a duplicated line
  (e.g. a chorus repeated twice) from having its two occurrences' timing
  swapped: matched pairs must be strictly increasing in both old and new
  indices, so the first old occurrence can only ever match the first
  surviving new occurrence. Whatever's left in the gaps between LCS
  anchors is paired positionally (`merge_edited_line`) - line-level timing
  always carries over; word-level timing only carries over per-word where
  both the count *and* that specific word's text are unchanged.
- **`TimedLine::new`/`resolve_timing` (the non-`_with_settings` versions)
  are marked `#[allow(dead_code)]`** - real production code (`main.rs`)
  always calls the `_with_settings` variant directly, so these simpler
  wrappers are only ever reached from tests in a normal build. Not a sign
  of unused/dead code to remove.

### `formats.rs` - lyric file import/export

**Purpose.** Reads and writes the three external lyric-file formats this
app understands (LRC, UltraStar, KOK), plus format auto-detection, so a
user can start from a file that already carries real timing instead of
always pasting plain text. Import produces `Vec<LyricLine>` (the same
type manual paste produces); export is the direct inverse, consuming
`&[TimedLine]` the same way `export.rs`/`video.rs` do.

**Key items:**

- `LyricFormat` (`PlainText`/`Lrc`/`UltraStar`/`Kok`) and
  `detect_format(text) -> LyricFormat` - sniffs the first ~4000 characters
  for each format's own distinctive marker (`#TITLE:`/`#BPM:` for
  UltraStar, a `[mm:ss]`-shaped tag for LRC, a `digits,digits;` token for
  KOK), falling back to `PlainText` - not a failure case, a valid format
  in its own right.
- **LRC**: `parse_lrc_time_tag` (handles `[mm:ss.xx]`, bare `mm:ss`, and
  `mm:ss:xx`), `parse_lrc2_inline_tags` (extracts `<mm:ss.xx>` word tags),
  `import_lrc`/`export_lrc` (LRC1 line-level or LRC2 word-level, chosen by
  an `enhanced: bool` flag).
- **UltraStar**: `ultrastar_beat_to_secs`/`secs_to_beat` (UltraStar's beat
  format is 4x the "musical" BPM by long-standing convention - see the
  function's own doc for the exact formula), `import_ultrastar` (returns
  `Result<Vec<LyricLine>, String>` - a genuinely malformed file, missing
  `#BPM:`, is a real error, not silently empty output), `export_ultrastar`
  (writes a fixed notional BPM, `EXPORT_ULTRASTAR_BPM` = 400, chosen purely
  for round-trip precision - see Gotchas).
- **KOK**: `parse_kok_time`, `import_kok` (word-level timestamps only;
  line breaks are a heuristic - see Gotchas).
- `import_plain_text` - a thin wrapper over `lyrics::parse_pasted_lyrics`,
  so "import by detected format" has one consistent entry point even for
  the plain-text case.

**Callers/callees.** Called by `main.rs`'s `load_lyrics_file` (import) and
`start_combined_export` (export). Calls into `lyrics.rs` for `LyricLine`/
`Singer`/`TimedLine`/`word_timings`/`parse_pasted_lyrics`.

**Non-obvious design decisions and gotchas:**

- **Every numeric parser here explicitly rejects non-finite values**
  (`is_finite()` plus a non-negative check) before accepting a parsed
  timestamp/BPM/GAP. This isn't incidental - Rust's `f64::parse` accepts
  the literal strings `"nan"`/`"inf"`/`"infinity"` (case-insensitively) as
  valid floats, and without this guard a crafted file (`[00:nan]`, a KOK
  `nan;word;` token, `#BPM:nan`) would produce a `NaN` that later poisons
  a `sort_by(...).unwrap())` call elsewhere and panics the app the instant
  the file is opened. `parse_lrc_time_tag`, `parse_kok_time`, and the
  UltraStar `#BPM:`/`#GAP:` parsing all guard against this independently
  - see each function's own comment for the exact mechanism, and see
  `lyrics::parse_timecode` for the same pattern applied to manually typed
  timecodes.
- **KOK's line-break convention is a documented guess, not a confirmed
  spec.** No sample this project could verify demonstrated anything beyond
  word-level timing - `import_kok` groups words into lines heuristically
  (a pause over 1.2s, or 10 words, starts a new line). The word-level
  timing itself is reliable either way; only the line *grouping* might
  need manual re-adjustment after import.
- **UltraStar export's BPM (400) is not a tempo claim.** It's chosen for
  beat-resolution precision (37.5ms/beat) on the round trip through this
  app's own second-based timing, nothing about the song's actual musical
  tempo - any fixed BPM round-trips correctly as long as the same value is
  used consistently, since a reader only ever converts beats back to
  seconds via the file's own declared `#BPM:`/`#GAP:`.
- **UltraStar pitch data is parsed, then discarded.** Each note's pitch
  field is read (so the file's structure is understood/validated) but
  never stored - this app has no pitch-display feature, so an exported
  file writes a constant placeholder pitch instead.
- **`.kbp` (KaraokeBuilder Studio) is deliberately not implemented** -
  it's a complex, proprietary format this project doesn't have confident
  first-hand grammar knowledge of, and guessing at it risks silently wrong
  timing. See `CONTRIBUTING.md`'s "Adding a new lyric-file format" for
  what's needed to add it (or any other format) for real.

### `timeline.rs` - fine-tuning timeline math

**Purpose.** Pure time↔pixel mapping and drag-resolution math for the
fine-tuning timeline widget, with **zero dependency on egui or any other
module in this crate** - deliberately kept this way so it can be unit-
tested without a running GUI context. `main.rs`'s `draw_timeline` owns the
actual widget: painting, hit-testing, and wiring drag results back into
`LyricLine` fields all live there, not here (see the `main.rs` chapter).

**Key types/functions:**

- `View` (`px_per_sec`, `scroll_secs`) - `time_to_x`/`x_to_time` convert
  between song-seconds and screen-pixels; `zoom_at(factor, area_left,
  anchor_time)` zooms while keeping a specific time fixed under the
  cursor; `pan_by_pixels` scrolls, clamped to never go negative.
  `MIN_PX_PER_SEC`/`MAX_PX_PER_SEC` (4.0/400.0) bound the zoom range.
- `tick_interval_secs(px_per_sec, min_px)` - picks the smallest "nice"
  ruler interval (from a fixed candidate list: 0.1s up to 1800s) whose
  on-screen spacing is still at least `min_px` apart, so labels never
  overlap regardless of zoom.
- `DragMode` (`Body`/`LeftEdge`/`RightEdge`) and `classify_drag(local_x,
  width_px)` - which part of a bubble a drag at a given local x-offset
  should affect; the edge-grab zone (`EDGE_GRAB_PX` = 8.0) shrinks for
  bubbles narrower than `2 * EDGE_GRAB_PX` so both edges stay reachable on
  a tiny bubble instead of one edge's threshold swallowing the whole
  thing.
- `DragBounds` (`min_start`/`max_sing_end`) and `drag_bounds(prev_sing_end,
  next_start)` - the range a drag may move within, derived from the
  *neighboring* lines so a drag can never reorder lines or eat into a
  neighbor's time.
- `apply_body_drag`/`apply_left_edge_drag`/`apply_right_edge_drag` - the
  actual clamped math for each `DragMode`, each guaranteeing at least
  `MIN_DURATION` (0.1s) of duration survives.
- `DragSession` (`mode`, `orig_start`, `orig_end`, `bounds`,
  `drag_start_pointer_x`) + `resolve(pointer_x, px_per_sec) ->
  (Option<f64>, Option<f64>)` - resolves the *current* pointer position
  against the drag's *original* captured state, returning `None` for
  whichever side this drag mode doesn't touch (so the caller only writes
  back fields that actually changed). Works identically whether the
  bubble represents a whole line (`start`/`sing_end`) or a single word
  (`highlight_at`/`held_until`) - the caller decides which.

**Callers/callees.** Called only by `main.rs`'s `draw_timeline`/
`TimelineDrag`. Calls nothing else in this crate.

**Non-obvious design decisions and gotchas:**

- **`DragSession::resolve` is stable across repeated calls at the same
  pointer position** - calling it twice with no mouse movement between
  gives identical results (there's a dedicated test for exactly this).
  This is what makes resolving from the *original* captured state safe to
  do every single frame of a drag, rather than needing to track "how much
  changed since last frame" and risk accumulating rounding drift.
- **Word-level drag bounds must come from the whole *line's* window, not
  the immediate neighboring word.** A regression test
  (`word_body_drag_has_room_to_move_when_bounded_by_the_whole_line`) locks
  this in: words default to a gapless, continuous wipe, so a neighboring
  *word's* own position touches this word's edge exactly - bounding a
  drag by that would leave zero room to move the word at all. `main.rs`
  bounds a word drag by its *line's* `[start, sing_end]` instead,
  specifically to avoid this.

### `project.rs` - project files and crash recovery

**Purpose.** Two related jobs: serializing the whole session to a
`.abyzl` JSON file (and back) for "Save Project"/"Load Project…", and
managing the crash-recovery autosave slot - a *separate*, fixed-location
file using the exact same on-disk format, written periodically and read
back at startup.

**Key items:**

- `FILE_EXTENSION` ("abyzl"), `CURRENT_VERSION` (1 - bumped only for
  non-additive format changes; a new optional field doesn't need a bump,
  see Gotchas).
- `RgbColor` (plain 8-bit RGB, no `egui` dependency) and `ProjectColors`
  (one `RgbColor` pair per `Singer` variant, plus background/preview/
  title/artist) - the serialized form of every customizable color.
- `ProjectFile` - the complete saved-session struct: `version`,
  `audio_path`, `lyrics_raw`, `lines: Vec<LyricLine>`, `title`, `artist`,
  `colors`, `video_resolution`, `video_codec`, `show_credit_line`,
  `background`, `background_fit`, `background_dim`, `lyric_font_family`,
  `timing_settings`. `save_to_file`/`load_from_file` are plain
  `serde_json` round trips.
- `ensure_project_extension(path)` - appends `.abyzl` if a path doesn't
  already end in it (case-insensitively) - a safety net since not every
  native file-save dialog reliably appends a custom extension on its own.
- **Autosave**: `AUTOSAVE_FILENAME` ("autosave.abyzl"), `autosave_path`
  (via `eframe::storage_dir(app_id)` - the OS's own per-user app-data
  directory, the same one `eframe`'s window-position persistence already
  uses), `write_autosave`/`read_autosave`/`clear_autosave`. `read_autosave`
  swallows any error (missing file, corrupt JSON) into `None` rather than
  surfacing it - "nothing to recover" is the right read for either case.

**Callers/callees.** Called extensively by `main.rs` (`to_project_file`/
`apply_project_file`, `write_project_to`, `load_project_file`, the
periodic-autosave check in `update`, `on_exit`). Depends on `lyrics.rs`
(`LyricLine`, `TimingSettings`) and `video.rs` (`Background`,
`BackgroundFit`, `Resolution`, `VideoCodec`).

**Non-obvious design decisions and gotchas:**

- **A dedicated `default_true()` function, not a bare `#[serde(default)]`,
  for `show_credit_line`.** Serde's `#[serde(default)]` on a missing field
  calls `Default::default()`, which for `bool` is `false` - exactly wrong
  for a flag that should default *on* for every project saved before it
  existed. Any future boolean field that should default to `true` needs
  the same treatment, not the bare attribute.
- **`ProjectColors`'s `default_unsung`/`default_highlight` are
  `Option<RgbColor>`, not plain `RgbColor`** - a project saved before
  `Singer::Default` had its own independent color simply omits these
  fields, and `#[serde(default)]` makes them deserialize to `None`. The
  fallback itself (render as `Male`'s colors, matching the old behavior
  exactly) happens at the *caller* level, in `main.rs`'s
  `apply_project_file` - `project.rs` itself doesn't know or care what the
  fallback should be, it just faithfully represents "this field wasn't
  saved."
- **The autosave is a completely separate file from any project the user
  explicitly saved** - writing one never touches the other. This is *why*
  "Save Project" alone can't prevent a stale autosave from lingering (see
  the `main.rs` chapter's note on `start_new_project` for the real bug
  this caused and how it was fixed).

### `recent.rs` - recently opened files

**Purpose.** A small, capped, most-recent-first list of previously opened
project and audio files, so reopening yesterday's work doesn't mean
hunting through a file picker again. Persisted next to the autosave slot,
same `eframe::storage_dir` mechanism.

**Key items:** `MAX_ENTRIES` (8), `RecentFiles { projects, audio }` (two
independent lists), `load`/`save`, `push_front` (removes any prior
occurrence of the same path, inserts at the front, truncates to
`MAX_ENTRIES`), `record_project`/`record_audio` (load → `push_front` →
save, swallowing write errors).

**Callers/callees.** Called by `main.rs` whenever a project or audio file
is opened/saved (`write_project_to`, `load_project_file`, `load_audio`),
and read by `draw_recent_files_menu`. Depends on nothing else in this
crate.

**Gotchas:** errors writing the list back to disk are silently swallowed
- worth doing best-effort, never worth interrupting the user's actual
save/load over a convenience feature failing to persist.

### `onboarding.rs` - first-run disclaimer flag

**Purpose.** The smallest file in this chapter: a single persisted
boolean, "has the legal disclaimer been acknowledged on this machine
already." Kept as its own tiny module rather than folded into
`RecentFiles`, since it isn't a "recently used files" concern at all.

**Key items:** `OnboardingState { disclaimer_acknowledged: bool }`,
`disclaimer_acknowledged(app_id) -> bool`, `acknowledge_disclaimer(app_id)`.

**Callers/callees.** `disclaimer_acknowledged` is read once, in
`KaraokeApp::new()`, to decide whether `show_disclaimer_modal` starts
`true`; `acknowledge_disclaimer` is called from the modal's "I
Understand" button. Depends on nothing else in this crate.

**Gotchas:** a missing *or corrupt* state file is treated identically to
"not yet acknowledged" - showing the disclaimer again is the safe
default on any doubt, never a silent skip.

## Audio, DSP & ML

This chapter covers everything between "a loaded audio file" and "a
clean vocal/instrumental split" or "word-level timing derived from real
speech recognition" - the two features in this app that are genuine
machine learning, not heuristics. Three ideas are worth understanding in
plain language before the code, because the rest of this chapter assumes
them.

**Spectrograms and the STFT.** A raw audio file is just a long list of
numbers (samples) - amplitude over time. That's not a useful shape for
most ML models to work with, because "what frequency is playing right
now" isn't visible in a single sample, only in how a *window* of
consecutive samples wiggles. The **Short-Time Fourier Transform (STFT)**
slides a window across the signal and, for each position, runs a Fourier
transform (a well-known decomposition of a window of sound into its
frequency components) - the result is a 2D grid: time along one axis,
frequency along the other, each cell a complex number encoding "how much
of this frequency, with what phase, was present in this time window."
That grid is a spectrogram, and it's the actual input format the vocal-
separation model was trained on. Going back from a modified spectrogram
to real audio (the **inverse STFT**) means overlapping each window's
reconstructed samples and blending them together (**overlap-add**) -
simple in concept, but exactly matching the reference implementation's
windowing/normalization convention is what `stft.rs` is really about.

**Vocal/instrumental separation (MDX-Net).** The idea: feed a short chunk
of the song's spectrogram into a neural network trained to output *just
the instrumental* (or, for one model here, just the vocals) as its own
spectrogram, then inverse-STFT that back to audio. The *other* stem isn't
a second model run - it's derived by simple subtraction from the
original mix, which is free once you already have one stem. Doing this
for a whole song means chunking it (the model expects a fixed-size input)
and overlap-adding the chunks' outputs back together, the same overlap-
add idea as the inverse STFT, just at the chunk level instead of the
window level.

**Forced alignment (CTC).** Given audio *and* the exact words being sung
(this app already knows the lyric text - there's no transcription
ambiguity to resolve), forced alignment finds *when* each word occurs. A
speech-recognition model trained with **CTC (Connectionist Temporal
Classification)** outputs, for every short time frame, a probability
distribution over "which character (or blank/pause) was spoken right
now." Finding the single best way to walk through those frame-by-frame
probabilities while spelling out the *known* target text in order is a
dynamic-programming problem - a grid (**trellis**) where one axis is
frames and the other is position in the target text, each cell holding
the best score reaching that point, solved forward then read backward
(**backtracked**) to recover exactly which frames each word landed on.

With that grounding, the files below (presented in dependency order, not
the order first listed, the same approach the Data Model chapter took):

### `audio.rs` - playback and the tap-to-time clock

**Purpose.** Audio decoding/playback (via `rodio` + `symphonia`) and a
deliberately decoupled wall-clock position tracker, used both for simply
playing the song and for tap-to-time (pressing a key in sync with
playback to set a lyric line's timestamp).

**Key items:** `probe_duration_secs(path)` - reads a file's total length
without decoding the whole thing. `PlaybackClock` (private) - pure
`base_position`/`running_since: Option<Instant>` bookkeeping with
`play_from`/`pause`/`resume`/`seek`/`stop`, no audio device involved at
all. `AudioPlayer` - the real thing `main.rs` holds: wraps a `rodio`
`OutputStream`/`Sink` plus one `PlaybackClock`, with `load`/`play_from_start`/
`seek`/`pause`/`resume`/`stop`/`position`/`duration`/`is_playing`/
`finished_naturally`.

**Callers/callees.** Held and driven by `main.rs` (`KaraokeApp::audio`).
Calls `rodio`/`symphonia` directly; nothing else in this crate.

**Gotchas:**
- **`PlaybackClock` is split out specifically so play/pause/seek logic is
  unit-testable without real audio hardware** - every test in this file
  exercises the clock directly, not `AudioPlayer`, which needs a working
  output device CI may not have.
- **`seek` deliberately doesn't use `rodio`'s own `Sink::try_seek`** - it
  silently no-ops (returns `Ok(())` without moving anything) when the
  sink's internal sound counter reads zero, which this app hits in
  practice. Instead, `seek` rebuilds the whole decode pipeline from
  scratch and fast-forwards (`Source::skip_duration`) to the target -
  heavier per seek (it decodes-and-discards up to the target), but far
  more reliable across formats, and fast enough in practice that it's
  still effectively instant (decoding runs many times faster than real
  time).

### `waveform.rs` - timeline backdrop

**Purpose.** Decodes the loaded audio once into coarse min/max amplitude
buckets, cheap enough to redraw every frame even zoomed out over a whole
song - purely a visual aid so dragging a timeline bubble can be lined up
against an actual vocal onset instead of guessing blind.

**Key items:** `Waveform` (`bucket_duration_secs`, `peaks: Vec<(f32,
f32)>`), `build_waveform(path)` (mixes down to mono by averaging each
interleaved frame, buckets at a fixed `BUCKETS_PER_SEC` = 800.0 regardless
of the source's own sample rate/channel count), `peak_in_range(start,
end)` (aggregates every bucket touching a range - a single bucket at high
zoom, potentially thousands collapsed together at low zoom).

**Callers/callees.** `main.rs`'s `start_waveform_job`/`poll_waveform_job`
(on a background thread - decoding a multi-minute file takes a real
fraction of a second). Calls `rodio` directly for decoding.

**Gotchas:** bucketing is done by *frame* count, not raw interleaved
sample count - a stereo (or higher-channel) file would otherwise end up
with buckets `channels` times too short if sample count were used
directly.

### `ffmpeg_path.rs` - locating the bundled `ffmpeg`/`openh264`

**Purpose.** Finds the bundled `ffmpeg` binary (and the `libopenh264`
shared library its H.264 encoder loads) at runtime, the same "bundled
resource, else per-user cache, else download" shape `model_assets.rs`
uses for ML models - generalized here to native binaries instead of
model files.

**Key items:** `resolve_ffmpeg()` (bundled → cache → falls back to a
system `ffmpeg` on `PATH` - the *only* one of this project's bundled
binaries with that system fallback, since a system `ffmpeg` almost
certainly already has its own H.264 encoder), `resolve_openh264()`
(bundled → cache → downloads from Cisco's CDN, verified against a SHA-256
pinned in this same file, decompressed from `.bz2`), `command()` (builds
a `Command` with the right `DYLD_LIBRARY_PATH`/`LD_LIBRARY_PATH`/`PATH`
set so `ffmpeg` can find `libopenh264` wherever it ended up), `check_available()`.

**Callers/callees.** `video.rs` (every ffmpeg invocation) and `vocals.rs`
(`encode_via_ffmpeg`, for non-`.wav` stem output) call `command()`
exclusively - no caller anywhere constructs an `ffmpeg` `Command` any
other way. Calls `model_assets.rs` for the shared bundled/cache
resolution helpers and hash verification.

**Gotchas:**
- **`libopenh264` is deliberately *not* a copy this project compiles
  itself**, even though its own `ffmpeg` build links against openh264's
  headers at build time. Cisco's patent-royalty coverage for H.264 (the
  reason openh264 avoids the usual per-encoder patent licensing cost)
  applies only to *Cisco's own* separately-distributed binary - so the
  actual runtime library is downloaded from Cisco directly, not built
  from the same source `ffmpeg` links against for headers/ABI only.
- **The openh264 download is checked against a SHA-256 hash pinned in
  this file** (`openh264_download_sha256`), verified right after download
  and before the archive is ever decompressed - a real security fix made
  this session, not a defensive-programming reflex: this binary gets
  `dlopen`'d into the process, so an unverified download would mean
  running arbitrary native code from the network.
- **Getting the real ffmpeg binary onto a user's machine at all turned
  out to need two separate, non-obvious fixes this session**, neither
  visible from reading `ffmpeg_path.rs` alone: on macOS, Apple Silicon's
  hardened-runtime "library validation" rejected loading `libopenh264`
  unless an entitlements file explicitly disabled it (fixed in
  `Cargo.toml`'s packager config, not here); on Windows, the MinGW-built
  `ffmpeg.exe` silently failed to start at all on a machine without
  MSYS2 installed, because it dynamically linked the MinGW toolchain's
  own runtime DLLs by default (fixed with `-static` in the release
  workflow's build flags, not here either). Worth knowing when
  `ffmpeg`-related bugs only reproduce on a real end-user machine, never
  in CI or on a dev box.

### `model_assets.rs` - locating and verifying ML models

**Purpose.** The shared "find a bundled resource, else check the per-user
cache, else download it" resolver for every ONNX model file this app
uses (vocal separation, forced alignment) - plus, as of this session,
SHA-256 verification of anything freshly downloaded, before it's ever
trusted.

**Key items:** `bundled_resource_candidates(relative)` (every plausible
on-disk location relative to the running executable, across all 4
platforms' different packaging layouts - NSIS/WiX next to the exe, a
macOS `.app`'s `Resources`, Linux's `.deb`/AppImage `usr/lib/<pkg>`),
`user_cache_subdir(name)` (via the `dirs` crate), `resolve_model(filename,
download_url, expected_sha256)` (the main entry point - downloads only if
not found bundled or cached, then verifies before returning), `verify_sha256`/
`sha256_hex`, `download_to_file` (plain `ureq::get`, streamed to a
`.part` temp file then renamed).

**Callers/callees.** Called by `vocals.rs` (`load_separator_with_model`)
and `align.rs` (`Aligner::load`) for their respective model files, and by
`ffmpeg_path.rs` for its own `verify_sha256`/`download_to_file`/cache-dir
helpers (not `resolve_model` itself, which is model-specific). Calls
`sha2`/`ureq`/`dirs` directly.

**Gotchas:**
- **A file already found bundled or already sitting in the cache is
  never re-hashed.** `resolve_model`'s hash check only runs on a *freshly
  downloaded* file - re-verifying a >1GB alignment model's hash on every
  single app launch would add real, pointless latency for a check that
  only protects against tampering *during* the download itself. A file
  that fails verification is deleted immediately rather than left for a
  later run to trust.
- **This resolver's layout-guessing exists because it already got it
  wrong once in production** - the module's own docs reference a real
  CI-discovered bug (Linux's `.deb`/AppImage layout needing the crate's
  own name as an extra path segment, `usr/lib/<pkg>/`, not just
  `usr/lib/`) that cost real debugging time to track down. The long list
  of candidate paths exists specifically so this doesn't have to be
  guessed right on the first try for every future platform/packaging
  quirk either.

### `onnxrt.rs` - loading the ONNX Runtime library itself

**Purpose.** A separate concern from any *model* file: locates and loads
the ONNX Runtime shared library (via `ort`'s `load-dynamic` feature)
that both ML features need underneath them, once per process.

**Key items:** `ONNXRUNTIME_VERSION` ("1.28.0"), `dylib_filename()`
(platform-specific name), `ensure_loaded()` (the only function anything
outside this file calls - idempotent via a `OnceLock`, safe to call every
time a session is about to be created), `load()`/`commit(path)` (the
actual `ort::init_from` call).

**Callers/callees.** `MdxSeparator::load` and `Aligner::load` both call
`ensure_loaded()` before creating an ONNX Runtime session. Calls
`model_assets.rs` for the same bundled-or-cached resolution pattern,
generalized from a model file to a shared library.

**Gotchas:**
- **Exists specifically because ONNX Runtime publishes no prebuilt
  binary at all for one of this app's four release targets** (macOS
  x86_64, dropped upstream between versions 1.23 and 1.25) - rather than
  having three platforms link a downloaded binary at build time and one
  platform do something different, *every* platform goes through this
  one runtime-loading path uniformly (macOS x86_64's copy is built from
  source by the release workflow instead of downloaded, but loaded
  identically at runtime).
- **`commit`'s error formatting uses `{e:?}` (Debug), not `{e}`
  (Display)** - a real fix made this session, after a genuine macOS
  dlopen failure report came back as the useless string "dlopen failed"
  with no further detail. `ort`'s own error type's `Display` never prints
  the underlying OS-level reason, and its `source()` override doesn't
  expose it either - only the full `Debug` formatting recurses deep
  enough to surface the real `dlerror()` message. Worth remembering
  anywhere else in this codebase that wraps a third-party error for
  display: `{e}` can be strictly less informative than `{e:?}`.

### `stft.rs` - the short-time Fourier transform

**Purpose.** Forward and inverse STFT matching PyTorch's `torch.stft`/
`torch.istft` *exactly* (reflect-padding, a periodic Hann window,
NOLA-normalized overlap-add) - not "an" STFT implementation, but a
bit-faithful match to the specific convention the vocal-separation
model's weights were trained against. Any drift here (window shape,
padding, normalization) would degrade separation quality even with
the right model and the right `n_fft`/`hop_length`.

**Key items:** `Stft::new(n_fft, hop)`, `forward(signal) -> Spectrogram`
(reflect-pads by `n_fft/2` each side, then frames/windows/FFTs),
`inverse(frames) -> Vec<f32>` (windowed overlap-add, normalized by the
overlap-added *squared* window - the standard NOLA formula, needed since
`n_fft/hop = 6` isn't a power-of-two ratio that would make the window
trivially constant-overlap-add), `hann_window_periodic` (the *periodic*
Hann window - first sample 0, no repeated symmetric endpoint - distinct
from the *symmetric* Hann window `mdx.rs` separately uses for its own
chunk-level overlap-add), `reflect_pad` (mirrors around the edge sample
without repeating it, matching `numpy.pad(mode="reflect")`).

**Callers/callees.** Used only by `mdx.rs`. Calls `realfft` directly for
the actual FFT math.

**Gotchas:**
- **A model's raw output spectrum isn't guaranteed to have a purely real
  DC/Nyquist bin**, which a real-valued time-domain signal's spectrum is
  mathematically required to have. `realfft` enforces this strictly and
  errors on a stray imaginary component there; PyTorch's own `istft`
  silently tolerates/discards it. `inverse` zeroes those two bins'
  imaginary parts before handing them to `realfft`, specifically to match
  PyTorch's effective behavior rather than reject otherwise-fine model
  output - there's a dedicated test (`inverse_tolerates_a_stray_imaginary_
  part_on_the_dc_and_nyquist_bins`) using a deliberately "dirty" spectrum
  to lock this in.
- **Verified by round-trip reconstruction, not just "it compiles"** -
  `ISTFT(STFT(x)) ≈ x` within floating-point tolerance, against both a
  sine wave and pseudo-random noise, at MDX-Net's actual `n_fft`/`hop`.
  This is what gives real confidence the windowing/normalization
  convention matches PyTorch's, since a subtly wrong normalization
  constant would still "work" in the sense of not crashing.

### `mdx.rs` - the MDX-Net separation algorithm

**Purpose.** The actual vocal/instrumental separation math: chunked
overlap-add, STFT → ONNX model → ISTFT per chunk, and the
derive-the-other-stem-by-subtraction formula - a bit-faithful port of
`audio-separator`'s reference Python implementation (reverse-engineered
by reading that source directly, not reimplemented from a general
understanding of "an" MDX-Net), because matching that *exact* reference
matters more than matching the architecture in the abstract.

**Key items:** `MdxModel` (`InstHq3` default, `KimVocal2` alternative) -
each with its own verified `ModelParams` (`n_fft`, `dim_f`,
`segment_size`, `compensate`, and which `PrimaryStem` it outputs
*directly* - the other is always derived). `InstHq3`: `n_fft` 6144,
`dim_f` 3072, `segment_size` 256, `compensate` 1.022, outputs
Instrumental directly. `KimVocal2`: `n_fft` 7680, same `dim_f`/
`segment_size`, `compensate` 1.009, outputs Vocals directly instead.
`HOP_LENGTH` (1024, hard-coded - UVR uses this for every model it
publishes, confirmed directly in `audio_separator`'s own source, not a
genuine per-model value despite living next to ones that are).
`MdxSeparator::load`/`separate` (peak-normalizes the mix, calls `demix`,
derives the secondary stem by subtraction) /`demix` (the actual chunked
overlap-add loop, `OVERLAP` = 0.25) / `run_model` (one chunk through
STFT → zero the first 3 frequency bins → the ONNX model → ISTFT).

**Callers/callees.** `MdxSeparator` is constructed and driven entirely by
`vocals.rs`. Calls `stft.rs` for the transform and `onnxrt.rs`
(`ensure_loaded`) plus `ort` directly for inference.

**Gotchas:**
- **Every model parameter here was verified against the actual `.onnx`
  file's own hash** (UVR's MD5-of-last-10000KiB scheme), looked up in
  `TRvlvr/application_data`'s own `model_data_new.json` - not taken from
  a secondary description of either model. This is why there's a
  dedicated regression test
  (`every_model_has_its_own_verified_parameters`) locking these exact
  numbers in: a silent edit to any of them would produce wrong - not
  necessarily *crashing* - separated audio.
- **The subtraction formula uses the *normalized* mix, not the final
  peak-rescaled one** (`secondary = -primary*compensate + norm_mix`) -
  this looks like a scale mismatch unless the original peak was already
  ≤ 0.9 (in which case normalization was a no-op), but that's genuinely
  how `audio-separator`'s own default (`invert_using_spec=False`) path
  behaves, replicated as-is rather than "corrected" into different
  output.
- **Which array ends up "instrumental" vs. "vocals" depends on which
  stem the *loaded model* outputs directly** - `MdxModel::KimVocal2`
  flips this relative to the default `InstHq3`, so `Separated`'s two
  fields are never hard-wired to a specific array index.

### `vocals.rs` - the app-facing separation API

**Purpose.** The thin layer between `mdx.rs`'s algorithm and the rest of
the app: resolving which model file to load (bundled/cached/downloaded,
via `model_assets.rs`), decoding/resampling input audio to the model's
fixed 44.1kHz, and writing stems back out to WAV/MP3.

**Key items:** `load_separator()` (always `MdxModel::InstHq3` - the
"Instrumental audio"/"Vocals audio" export checkboxes and video's "Remove
vocals" all go through this one, unconditionally) vs.
`load_separator_with_model(model)` (auto-align's own selectable "Vocal
model" dropdown uses this instead - kept deliberately separate so
experimenting with a different model there can never change what an
export actually produces). `separate`, `separate_vocals_to_temp_wav`
(writes just the vocals stem to a temp file - what auto-align aligns
against). `write_stem_to_file` (WAV directly, or via `encode_via_ffmpeg`
for anything else). `load_stereo_44100`/`resample_to_44100` (via
`rubato`'s FFT-based resampler, only invoked if the source isn't already
44.1kHz).

**Callers/callees.** `main.rs`'s `start_combined_export` (export
checkboxes) and `start_word_alignment` (auto-align's own isolation step)
are the two callers. Calls `mdx.rs` for the algorithm, `model_assets.rs`
for model resolution, `ffmpeg_path.rs` (`encode_via_ffmpeg`) for non-WAV
output, `rubato` for resampling.

**Gotchas:**
- **One separation pass always produces *both* stems** - whichever one
  the loaded model doesn't output directly is derived by subtraction
  inside `mdx.rs`, not a second, independently expensive model run. This
  is why requesting "Instrumental audio," "Vocals audio," and video's
  "Remove vocals" together in one export only separates once rather than
  three times - `main.rs::start_combined_export` holds the one shared
  result in a local variable (`shared_separation`) and every output that
  needs a stem reads from it.
- **`separate_vocals_to_temp_wav`'s temp file is the caller's
  responsibility to delete** - it doesn't clean up after itself the way
  `write_stem_to_file`'s own internal `.wav.tmp` intermediate does.
  `main.rs`'s `start_word_alignment` removes it after alignment finishes
  (success or failure).

### `ctc.rs` - the forced-alignment trellis

**Purpose.** The pure dynamic-programming/tokenization math behind
`align.rs`, kept separate specifically so it's testable against
hand-built synthetic emission matrices with an unambiguous correct
answer, with no ONNX Runtime or real model dependency at all.

**Key items:** `Vocab` (`parse(json)` - reads a HuggingFace-style
`{"token": id, ...}` vocab, finding the blank token by matching
`<pad>`/`[PAD]` case-insensitively and the word-delimiter by the literal
`"|"` entry, since token spelling/ID isn't consistent across model
authors; `tokenize(word)` - NFC-normalizes and lowercases first, drops
any character with no vocab entry rather than mapping it to `<unk>`).
`TokenSpan` (`start_frame`/`end_frame`, half-open). `forced_align(emission,
tokens, blank_id)` - runs `build_trellis` then `backtrack`.
`log_softmax` (turns raw model logits into the log-probabilities
`forced_align` needs). `separate_repeats` (inserts an explicit blank
between adjacent identical tokens - see Gotchas).

**Callers/callees.** Called only by `align.rs`. Calls nothing else in
this crate.

**Gotchas:**
- **Adjacent identical tokens need an explicit blank inserted between
  them, or the trellis can't tell them apart** - the double "l" in
  "hello" looks identical to the DP as "one l held for longer" unless
  `separate_repeats` forces a blank between them first. `forced_align`'s
  caller (`align.rs`) is responsible for calling this before building the
  trellis; `ctc.rs` doesn't do it automatically.
- **Backtracking's tie-break (`stay` wins over `advance` unless `advance`
  is *strictly* greater, with a small epsilon) is deliberate, not
  arbitrary** - a frame equally consistent with "still holding the
  current token" and "just started the next one" is credited to the
  token already established, matching the reference forced-alignment
  algorithm's strict convention. Getting this backwards was a real bug
  caught during development (the backtrack initially preferred the wrong
  side of a near-tie, silently shifting every word boundary later than
  it should be) - see `backtrack`'s own doc comment.
- **A character with no vocab entry is dropped, never mapped to
  `<unk>`.** The model has near-zero real probability of emitting
  `<unk>` for an actual spoken syllable, so forcing one into the target
  sequence would only hurt alignment accuracy, not help it.

### `align.rs` - forced alignment orchestration

**Purpose.** Runs a wav2vec2-CTC speech model (via `ort`, loaded through
`onnxrt.rs`) once per already-timed, multi-word line, restricted to that
line's own tapped `[start, end)` window (plus a little padding), to fill
in real audio-derived word timing - "🪄 Auto-align words." Deliberately
per-line rather than one whole-song pass: each call only has to figure
out where *within a few seconds of audio* a handful of words fall, and a
mistimed line needs re-tapping regardless of how good alignment is.

**Key items:** `AlignLanguage` (9 variants: `Eng`/`Spa`/`Fra`/`Deu`/`Ita`/
`Por`/`Jpn`/`Kor`/`Cmn`, each with its own `vocab_json()` - see "Where
`assets/align_vocab` fits in" below - `model_filename()`/
`model_download_url()`/`model_sha256()`). `WordAlignment` (`start`/`end`).
`Aligner::load(audio_path, language)` (loads the vocab, resolves/downloads
the model via `model_assets.rs`, decodes the audio to mono 16kHz).
`align_line(words, window_start, window_end)` - the real entry point:
normalizes the audio window (`normalize_zscore` - zero-mean, unit-variance,
matching every bundled model's own `preprocessor_config.json`), runs the
model (`run_model`), converts raw logits to log-probabilities
(`ctc::log_softmax`), builds one flat target token sequence for every word
(with the vocab's word-delimiter between them), and calls
`ctc::forced_align`.

**Where `assets/align_vocab` fits in.** Each language's `vocab.json` is
embedded at compile time via `include_str!` (`vocab_json()` - the same
"small, git-tracked content baked into the binary" pattern `fonts.rs`'s
bundled fonts use), *not* bundled as a `cargo-packager` resource or
downloaded - only the (much larger, ~1.2GB) `.onnx` model files are
fetched separately. The files themselves are small (300 bytes for
English, which only needs the Latin alphabet, up to ~44KB for Mandarin);
`ctc.rs`'s `every_bundled_vocab_parses_and_has_a_blank_and_delimiter` test
checks every one of the 9 at once.

**Callers/callees.** Driven by `main.rs`'s `start_word_alignment`. Calls
`ctc.rs` for the trellis/tokenization, `onnxrt.rs`/`ort` for inference,
`model_assets.rs` for model resolution, `rubato` for resampling if the
input isn't already 16kHz mono.

**Gotchas:**
- **The 9 bundled models aren't all from the same, equally-established
  source.** English is `Xenova/wav2vec2-large-xlsr-53-english`; the other
  8 are `FinDIT-Studio`'s own ONNX conversions of the same base
  `jonatasgrosman`/`kresnik` checkpoints. Every conversion's fidelity was
  verified directly (not just trusted) by comparing the converted model's
  vocabulary against the original PyTorch checkpoint's `vocab.json` -
  an exact match across all 9, essentially impossible for an unrelated or
  mis-converted model to produce by chance - but this is also exactly
  *why* `model_assets::resolve_model`'s hash pinning matters most for
  these 6 specifically: a future compromise of that less-established
  uploader's account couldn't silently swap in a different model for
  someone who already verified and cached the real one.
- **This app isolates vocals *before* running alignment** (see the
  `main.rs` chapter's `start_word_alignment`) - a speech-recognition
  model, trained on clean speech, tracks singing far more reliably
  without instrumentation underneath it. If separation itself fails for
  any reason, alignment falls back to the original mixed audio rather
  than failing the whole run - it's an accuracy improvement, not a hard
  requirement.
- **Word-level results are best-effort, not guaranteed accurate** - a
  real speech model wasn't trained for singing over music, so results can
  be inconsistent on heavily melismatic/stylized vocals or overlapping
  voices (see `FEATURES.md`'s own "Auto-aligning word timing" section for
  the full caveat this app shows users directly).

## Rendering & Output

This chapter covers the two renderers that turn the same resolved
`Vec<TimedLine>` (from the Data Model chapter) into actual output files,
the in-app documentation system you may be reading this chapter in, and
the two layers of tooling - `build.rs` and `scripts/build-ffmpeg.sh` -
that produce things the renderers (and the rest of the app) depend on at
runtime. No new DSP/ML concepts here; CD+G, tile, and palette/CLUT are
all already defined in the Glossary.

### `cdg.rs` - the CD+G packet encoder

**Purpose.** The lowest-level piece of the `.cdg` pipeline: a byte-stream
writer with zero knowledge of lyrics, singers, or timing logic - just
"write this instruction, consuming this many packets' worth of time."

**Key items:** `PACKET_SIZE` (24 bytes), `PACKETS_PER_SEC` (300.0 - *the*
fact that makes CD+G timing work: there's no separate timestamp field in
the format, so timing is purely "how many packets precede this
instruction"). `TILE_COLS`/`TILE_ROWS` (50×18, the format's real canvas)
vs. `SAFE_COLS`/`SAFE_ROWS` (48×16, this app's own safety margin to avoid
the overscan border some players/TVs crop). `CdgColor` (4-bit-per-channel
RGB). `TilePixels` (`[u8; 12]` - one 6×12 tile, bit 5 = leftmost pixel).
`CdgWriter` - `memory_preset`/`border_preset` (clear the whole
screen/border to one color), `load_color_table` (loads 8 colors into
either palette half), `tile_block` (draws one tile at a `(row, col)` in
*safe* coordinates - automatically offset by `SAFE_ROW_OFFSET`/
`SAFE_COL_OFFSET`), `advance_to`/`pad_until` (the only ways time moves
forward), `into_bytes`.

**Callers/callees.** Used only by `export.rs`. Calls nothing else in this
crate - genuinely a leaf module, the lowest layer in the whole project.

**Gotchas:**
- **`advance_to` silently no-ops if the target is already in the past**
  (`pad_until` only ever adds filler packets, never removes any already
  written) - this is what lets `export.rs` issue draw calls slightly out
  of strict chronological order (e.g. a backing vocal's and a host
  line's word events interleaved) without corrupting the stream, as long
  as each call's *own* target time is monotonically non-decreasing
  overall.
- **Every instruction is one 24-byte packet, always** - `memory_preset`/
  `border_preset`/`load_color_table`/`tile_block` all funnel through the
  same private `push_packet`, so there's exactly one place that enforces
  the format's fixed packet shape (command byte `0x09`, instruction byte,
  4 bytes of unused parity, 16 bytes of instruction-specific data, 4 more
  parity bytes left as zero since software players ignore them).

### `font.rs` - CDG bitmap glyphs

**Purpose.** Renders a character into CD+G's native 6×12-pixel tile
format, sourced from the `font8x8` crate's 8×8 bitmap glyphs (the
left-most 6 of each row's 8 columns are used; the font's own rightmost 2
columns are unused padding).

**Key items:** `glyph_tile(ch) -> TilePixels` (an unsupported character
renders blank, same as a space - never a panic or a placeholder glyph).
`glyph_tile_scaled(ch, scale) -> Vec<Vec<TilePixels>>` - pixel-doubles the
base glyph into a `scale × scale` grid of real tiles, which is how the
current line renders bigger/more legible (2x) when it fits (see
`export.rs`). `reverse6` (font8x8's bit order is "bit 0 = leftmost
column"; CD+G's tile format is the opposite, "bit 5 = leftmost" - this
flips a 6-bit row between the two conventions, and is its own
involution: `reverse6(reverse6(b)) == b` for every possible byte, checked
directly by a dedicated test).

**Callers/callees.** Used only by `export.rs`. Calls `font8x8` directly.

**Gotchas:** this is a *fixed* 1-bit bitmap font with no scalable-font
machinery at all - there's no reasonable way to downsample an arbitrary
TrueType/OpenType font into something that still reads as letters at
6×12 pixels thresholded to 1 bit, which is *why* `fonts.rs`'s custom
system-font picker only applies to the video export/live preview (real
scalable-font renderers), never to `.cdg` - see `fonts.rs`'s own module
docs for the full reasoning.

### `export.rs` - the `.cdg` renderer

**Purpose.** Lays out lines, words, the title card, the countdown
indicator, and an optional backing vocal onto the CDG canvas using
`cdg.rs` + `font.rs`, driven by `lyrics.rs`'s timing - this is the module
that actually decides *where on the fixed 300×216 canvas* things go, not
just *when*.

**Key items:** `Palette` (one `CdgColor` pair per `Singer`, plus
background/preview/title/artist) and its 5 built-in presets
(`default`/`high_contrast`/`sunset`/`ocean`/`abyssal`) - see Gotchas for
the "abyssal" preset's accessibility reasoning. Fixed row layout:
`TITLE_ROW`/`ARTIST_ROW`/`LEGEND_ROW` (1/4/5), `CURRENT_ROW` (6, 2 rows
tall for 2x scale), `BACKING_ROW` (8), `PREVIEW_ROW` (10), `COUNTDOWN_ROW`
(13). `layout_line_scaled(text, preferred_scale)` - the "shrink if it
starts clipping" logic: tries `PREFERRED_SCALE` (2x), automatically falls
back to 1x if the text wouldn't fit the 48-column safe width, and clips
as an absolute last resort even at 1x. `HighlightEvent` + the merged,
time-sorted event list that interleaves a backing vocal's word-wipe with
its host's (see Gotchas). `title_card_end(timed_lines)`,
`paired_audio_path`/`copy_paired_audio` (the "MP3+G" convention - copying
the loaded audio next to the export under a matching base filename).
`render_cdg_with_settings` - the actual top-to-bottom renderer.

**Callers/callees.** Called by `main.rs`'s `start_combined_export`. Calls
`cdg.rs`, `font.rs`, and `lyrics.rs` (`word_timings`, `countdown_window`,
`singer_legend`, `effective_singer_label`, and more).

**Gotchas:**
- **A backing vocal's and its host line's word-highlight events must be
  emitted in one merged, time-sorted sequence, not "all of one, then all
  of the other."** Since `CdgWriter::advance_to` only ever moves forward
  (see the `cdg.rs` section above), naively emitting the host's full word
  list followed by the backing vocal's would silently drop every backing-
  vocal event whose time falls *before* the host's last event - `render_cdg_
  with_settings` builds one combined `Vec<HighlightEvent>` across both rows
  and sorts it by time before emitting anything, specifically so the two
  wipes can run concurrently (the whole point of a backing vocal) without
  corrupting the stream.
- **The CDG palette is 16 slots, and this app uses 14 of them, not
  16** - 8 via the low CLUT (background + 3 voice pairs + preview), 6 via
  the high CLUT (title + artist + screaming pair + default pair, leaving
  2 of that half's 8 slots genuinely free for future use). There's no
  "Default" voice slot hiding in the low CLUT - it lives in the high one,
  alongside title/artist/screaming.
- **The "Abyssal" preset's color choices are a deliberate *lightness*
  contrast, not a hue contrast** - every voice pair is a clearly dim tone
  against a clearly bright one, and no pair relies on a red-vs-green
  axis (the one combination that fails hardest under the most common
  forms of color blindness). See `Palette::abyssal`'s own doc comment for
  the exact reasoning per pair - this wasn't an incidental color choice.
- **Every CDG color is only ever an *approximation* of a "nice" hex
  value** - 4 bits per channel means 16 possible levels, not 256, so even
  a carefully chosen preset can be off by a few values per channel from
  its nominal target. This is a real ceiling of the format itself, not a
  precision bug in this renderer.

### `fonts.rs` - system font loading for video/preview

**Purpose.** Enumerates installed system fonts and loads one's raw bytes,
for the optional custom lyric-text font - scoped to the video export and
live preview only (both real scalable-font renderers), never `.cdg` (see
`font.rs`'s Gotchas above for why).

**Key items:** `BUNDLED_FONTS` (`Creepster`, `Nosifer` - both SIL Open
Font License 1.1, baked into the binary via `include_bytes!`, merged into
the picker's results alongside whatever's actually installed on the
machine). `list_family_names()` (via `font-kit`'s `SystemSource`,
deduplicated/sorted - can take a noticeable moment on a machine with many
fonts, meant to run on a background thread). `load_family_bytes(name)` -
checks the bundled list first, else asks `font-kit` for the real family
and picks whichever face scores closest to "normal" (upright, weight 400)
via `normal_face_distance`, since a family's enumeration order isn't
guaranteed to put the regular weight first.

**Callers/callees.** Called by `main.rs` (`start_font_list_job`,
`ensure_font_loaded`). Results feed into `video.rs`'s `render_video`
(`custom_font_bytes`) and the live preview. Calls `font-kit` directly.

**Gotchas:** `font-kit`'s own enumeration can repeat a family name once
per style/weight file on some platforms/backends - `dedup_sorted`
exists specifically to collapse that before it ever reaches the picker
UI, not just for a tidier alphabetical order.

### `video.rs` - the `.mp4` renderer

**Purpose.** An independent frame-by-frame RGB24 renderer (anti-aliased
text via `ab_glyph`, embedded DejaVu Sans/Sans Bold by default) piped
into a bundled `ffmpeg` subprocess, driven by the *same* `lyrics.rs`
timing `export.rs` uses - but with real room to work with (arbitrary
resolution, no 16-color ceiling), so it shows a whole verse-style block
of lines at once instead of just one current line.

**Key items:** `Resolution` (`Hd1080`/`Uhd4k`), `VideoCodec`
(`Av1` default via SVT-AV1, CRF-based; `H264` via openh264, fixed-bitrate,
kept for compatibility - see the DSP/ML chapter's `ffmpeg_path.rs` section
for *why* two codecs). `Background` (`Image`/`Video`) and `BackgroundFit`
(`Cover`/`Contain`). `Rgb8`/`VideoPalette` (full 8-bit color - no 4-bit
ceiling here). `Canvas` (a plain RGB24 buffer with `fill`/`set_from_rgb24`/
`dim`/`blend_pixel`). `draw_text_line_chars` (per-character color, used
for the singer legend), `draw_text_line_wipe` (the actual continuous
color-wipe - blends `highlight`/`unsung` across a few pixels right at the
wipe boundary rather than a hard per-letter snap), `draw_text_line_uniform`
(one flat color, with an `opacity` multiplier for fades).
`render_frame` - the single function that decides what one frame at time
`t` looks like: title card, intro countdown, the current block (sung/
singing/upcoming slots, each independently faded via `lyrics.rs`'s
fade functions), the rolling hand-off into freed slots, the countdown
indicator. `fit_filter` (an `ffmpeg` `-vf` filter string for
Cover/Contain). `load_image_background`/`spawn_background_video_decoder`/
`extract_video_background_thumbnail` (the three ways a background
actually gets pixels). `render_video` - spawns the real encoding
`ffmpeg` process, loops over every output frame, writes each one to its
stdin.

**Callers/callees.** `render_video`/`extract_video_background_thumbnail`
called by `main.rs`; `render_frame`'s math is shared with the live
preview the same way export/preview/video all share `lyrics.rs`'s own
timing functions (not by the live preview calling into `video.rs`
directly) - see the "How the Modules Talk" chapter's "How data crosses
module boundaries" section. Calls `lyrics.rs` extensively, `ab_glyph`
for text, `ffmpeg_path.rs`'s `command()` for every `ffmpeg`
invocation, `image` for still backgrounds.

**Gotchas:**
- **A real AV1 background video used to hang indefinitely**, retrying
  hundreds of times with "Your platform doesn't support hardware
  accelerated AV1 decoding" - not fixable by any `-hwaccel` CLI flag,
  since this app's own `ffmpeg` build had no *software* AV1 decoder at
  all before `dav1d` was added (see the `scripts/build-ffmpeg.sh` section
  below for the real root cause). `-hwaccel none` is still set on both
  background-video `ffmpeg` invocations as a belt-and-suspenders measure,
  not the actual fix.
- **A background video's thumbnail extraction retries at several seek
  points, not just frame 0** (`THUMBNAIL_SEEK_CASCADE`: 0.5s, 3s, 8s,
  15s, then frame 0 itself) - some real-world files (confirmed directly:
  one downloaded/remuxed via a tool like `yt-dlp`) have a leading stretch
  that fails to decode at all (a spliced-in title card/fade-in with
  different stream parameters from the rest of the file), which makes a
  single fixed seek come up empty even though the rest of the file is
  perfectly fine.
- **A later background-video frame decode hiccup just freezes on the
  last good frame rather than failing the whole export** - only a
  failure to decode the *first* frame is treated as fatal. A multi-
  minute export shouldn't abort over one transient glitch near the end.
- **`render_frame` does *not* touch the background** - the caller
  (`render_video`'s own per-frame loop) fills the canvas first (flat
  color, a decoded image, or the next video frame, then optionally
  dimmed), and `render_frame`'s own drawing is all `blend_pixel`-based
  specifically so it composites correctly over either case without
  needing to know which one it is.

### `codex.rs` - the in-app documentation browser

**Purpose.** The window you may be reading this very paragraph in: a
`CommonMarkViewer`-rendered sidebar-plus-article browser over a small set
of this project's own existing docs, embedded at compile time so there's
no runtime path to get wrong and no second copy of any doc's content to
drift out of sync.

**Key items:** `Article` (`title`/`category`/`content`, all `&'static
str`). `ARTICLES` - the small static table (Features, Architecture,
Build Troubleshooting, the Legal Disclaimer, Changelog), each
`include_str!`'d straight from its real root-level `.md` file.
`DISCLAIMER_TEXT` - `DISCLAIMER.md`'s content, embedded once and reused
both as a Codex article *and* as `main.rs`'s first-run disclaimer modal,
so there's one `include_str!` site for it, not two. `LEARN_THE_CODE_TEXT`
- this very file, embedded the same way. `split_markdown_chapters(md)` -
splits a markdown string on `"## "` heading boundaries into one `Article`
per chapter (plus an "Overview" article for any content before the first
heading), all under a shared `"Learn the Code"` category - see "Why
`LEARN_THE_CODE.md` is split into several articles" below.
`all_articles()` - the combined, `OnceLock`-cached list `main.rs`'s
`draw_codex` actually iterates: `ARTICLES` plus
`split_markdown_chapters(LEARN_THE_CODE_TEXT)`.

**Callers/callees.** `all_articles()`/`DISCLAIMER_TEXT` are read by
`main.rs` (`draw_codex`, `draw_disclaimer_modal`). Calls nothing else in
this crate - it has no dependency on `egui_commonmark` itself either;
that rendering happens entirely in `main.rs`.

**Why `LEARN_THE_CODE.md` is split into several articles, unlike every
other article here.** Every other doc this module embeds (`FEATURES.md`,
`ARCHITECTURE.md`, etc.) is short enough to read as one scrolling pane.
This guide isn't - it's a code-level, file-by-file walkthrough, long
enough that showing it as a single article would mean scrolling through
the whole thing to reach any one chapter, since `egui_commonmark` 0.17
has no anchor-link/jump-to-heading support at all (checked directly
against its source before deciding this, not assumed). Splitting on `"##
"` headings gets a real chapter sidebar instead, while `docs/LEARN_THE_
CODE.md` stays exactly one file on disk - the same "one real place this
content lives" guarantee every other article already has, just
implemented with one extra step for this one doc.

**Gotchas:**
- **The split only recognizes headings at exactly `"## "` (level 2)** -
  a `"### "` (level 3) heading inside a chapter is correctly *not* a
  split point (there's a dedicated test for this), which is why every
  chapter in this guide uses `###` for its own internal subsections
  rather than `##`. Using `##` for a subsection by mistake would silently
  fragment that chapter into several separate Codex entries.
- **`Article`'s `title`/`category`/`content` are all literally `&'static
  str` byte-slices of the one embedded `LEARN_THE_CODE_TEXT` constant**,
  not owned `String`s - splitting never allocates new text, only
  computes new start/end byte offsets into the same compiled-in data.
  This is what makes `all_articles()`'s `OnceLock` cache cheap to build
  once and keep for the life of the process.

### `build.rs` - Windows icon/version embedding

**Purpose.** A `cargo build.rs` script, not part of the app's own
runtime code at all - on Windows only, it embeds the app icon plus basic
version/description metadata directly into the compiled `.exe`'s own PE
resources, via the `winresource` crate.

**Key items:** checks `CARGO_CFG_TARGET_OS == "windows"` and no-ops
entirely otherwise. Sets the icon from `assets/icons/abyssal-cdg-icon.ico`,
overrides `ProductName`/`FileDescription` (winresource's own default
`ProductName` is the crate name, `"abyssal-cdg"`, not this app's real
display name). A failure here is reported as a `cargo:warning`, not a
hard build error - a broken taskbar/Alt-Tab icon isn't worth failing the
whole Windows build over.

**Callers/callees.** Invoked automatically by `cargo build` on every
Windows build (that's what a `build.rs` is - no explicit call site).
Calls `winresource` directly.

**Gotchas:** this is a *separate* concern from `cargo packager`'s own
`icons` config in `Cargo.toml`, which only sets the *installer's* icon
(the `.msi`/setup `.exe`) and Start Menu shortcut - `cargo packager`
never touches the compiled binary's own PE resources. Without `build.rs`,
the installed app's actual `.exe` (taskbar, Alt-Tab, "Open File
Location," Add/Remove Programs) would show Windows' generic executable
icon regardless of what the installer itself looks like.

### `scripts/build-ffmpeg.sh` - building the bundled `ffmpeg`

**Purpose.** Builds this project's own LGPL-only `ffmpeg` (no x264/x265)
from source, along with everything it links against: `openh264`
(headers/ABI only - see the DSP/ML chapter's `ffmpeg_path.rs` section for
why the *runtime* library is a separate download), LAME (MP3 encoding),
dav1d (software AV1 *decoding*), and SVT-AV1 (AV1 *encoding*). Run by the
release workflow for all 4 platforms from one script, with platform
differences handled entirely through environment variables rather than
forking the script itself.

**Key items/pinned versions:** `FFMPEG_VERSION` (`n7.1`), `LAME_VERSION`
(`3.100`), `DAV1D_VERSION` (`1.4.3`), `SVT_AV1_VERSION` (`v2.3.0` -
pinned *below* v3.0.0 deliberately, see Gotchas), `OPENH264_VERSION`
(`2.6.0`, must match `ffmpeg_path.rs`'s own pinned version exactly, since
the two need to agree on the C ABI). `OPENH264_OS` auto-detection -
reproduces openh264's own Makefile's exact `uname`-based formula rather
than guessing a platform name independently. Per-library build steps in
order: openh264 (built, then discarded - never bundled), LAME (static),
dav1d (static, via Meson), SVT-AV1 (static, via CMake), then `ffmpeg`
itself (`--disable-autodetect --disable-gpl --disable-nonfree
--enable-static --disable-shared`, plus `--enable-libopenh264`/
`--enable-libmp3lame`/`--enable-libdav1d`/`--enable-libsvtav1`).

**Callers/callees.** Run by `.github/workflows/release.yml` for all 4
release targets (see the "Build, Test, CI & Release" chapter). Not
invoked by `cargo build`/`cargo test` at all - none of its output is a
build-time dependency of this crate.

**Non-obvious design decisions and gotchas (this script accumulated
several real, CI-discovered fixes - each is worth knowing before
touching the build):**
- **`SVT_AV1_VERSION` is pinned below v3.0.0 because v3.0.0 made a
  breaking API change** (`svt_av1_enc_init_handle`'s signature, and a
  renamed/removed config field) that the pinned `ffmpeg` version's own
  `libsvtav1.c` wrapper (written before that change) doesn't know about
  - a newer SVT-AV1 tag fails with a real compile error, not something
  fixable by any flag. The `ffmpeg` version and the SVT-AV1 version are
  coupled to each other, not independently upgradable without also
  patching `libavcodec/libsvtav1.c` to match.
- **`--disable-autodetect` is load-bearing, not tidiness** - without it,
  `./configure` silently links against whatever matching-named codec
  libraries happen to already be installed on the *build machine*,
  discovered the hard way when a build host with its own system ffmpeg/
  codec packages produced a binary linked against *those* instead of the
  LGPL-only ones this script just built, including at least one nonfree
  codec none of this script's own flags requested.
- **`openh264`'s own build bakes a *versioned* library name into what it
  produces** (`libopenh264.so.8`, or an absolute build-machine path on
  macOS) - harmless for openh264's own releases, but this copy is never
  actually shipped (only linked against for headers/ABI), so what
  matters is that `ffmpeg`'s own build records a *simple, fixed* expected
  filename matching exactly what gets bundled/downloaded at runtime. Linux
  gets this via a Makefile override (`SHAREDLIBSUFFIXMAJORVER=so`, with a
  self-referential-symlink side effect fixed up immediately after);
  macOS needs a different fix (`install_name_tool -id`, since its
  install-name is an absolute build path, not a simple suffix) - verified
  end-to-end on Linux with `readelf` and a real build+link+run+encode
  test against Cisco's actual binary swapped in; the macOS path is
  written from documented flags but hasn't been run outside this
  project's own CI.
- **`USE_ASM=No` for openh264 is scoped to macOS arm64 *only*, not
  applied more broadly** - its NEON assembly failed to compile on the
  GitHub Actions macOS arm64 runner's toolchain (a known openh264/Apple
  Silicon issue). Disabling it for the macOS x86_64 cross-compile target
  instead *broke that build*: openh264's own Makefile only adds the
  `-arch x86_64` flag inside its `USE_ASM=Yes` branch, so disabling asm
  there silently dropped the architecture flag entirely, producing a
  host-arch library that `ffmpeg`'s own link-time check correctly
  rejected as "not found." A real example of a fix for one platform
  breaking a different one, caught by a real CI run, not predicted in
  advance.
- **Every `eval ./configure ...` (not a plain unquoted expansion) exists
  so a value like `FFMPEG_CONFIGURE_EXTRA`/`LAME_CONFIGURE_EXTRA` can
  itself contain a quoted, space-containing argument** (e.g.
  `--cc="clang -arch x86_64"` for the macOS cross-compile case) and have
  that quoting actually respected - a plain unquoted expansion word-splits
  on every space regardless of quotes, which is exactly what broke the
  first real CI run of the macOS cross-compile case.
- **The Linux recipe (no cross-compile flags at all) is the one that's
  actually been run and verified end-to-end**; the macOS cross-compile
  and Windows/MSYS2 paths are written from each tool's own documented
  flags but hadn't been exercised for real outside this project's own CI
  until the release workflow itself ran them - see the script's own
  top-of-file comment for the exact scope of what's verified vs. what's
  "should work, per the docs."

## How the Modules Talk

The previous three chapters covered every module on its own. This one
covers the connective tissue: literally which module imports which
(verified directly against every real `use crate::`/`crate::module::`
reference in the source, not inferred from descriptions), how the
handful of shared data types cross those boundaries, and how background
work and the UI stay in sync.

### The dependency graph

`main.rs` depends on all 22 other modules (it declares every one of them
as `mod ...;`). Everything *below* `main.rs` forms a clean, acyclic,
6-layer graph - no module ever depends on one above it:

```
Layer 0 (zero dependencies on anything else in this crate):
  audio  cdg  codex  ctc  fonts  lyrics  model_assets
  onboarding  recent  stft  timeline  waveform

Layer 1 (depend only on Layer 0):
  font         -> cdg
  formats      -> lyrics
  ffmpeg_path  -> model_assets
  onnxrt       -> model_assets

Layer 2 (depend only on Layers 0-1):
  mdx     -> stft, onnxrt
  export  -> cdg, font, lyrics
  align   -> ctc, model_assets, onnxrt

Layer 3 (depend only on Layers 0-2):
  vocals  -> mdx, model_assets, ffmpeg_path
  video   -> lyrics, export, ffmpeg_path

Layer 4:
  project -> lyrics, video

Layer 5:
  main.rs -> every module above
```

A few of these are worth calling out specifically, because they're easy
to miss just from reading each file's own module doc:

- **`video.rs` depends on `export.rs`** - not just on `lyrics.rs`.
  `render_video` calls `export::title_card_end` directly (`let card_end =
  crate::export::title_card_end(timed_lines);`) rather than
  recalculating when the title card ends itself. This is exactly the
  kind of single-source-of-truth sharing `ARCHITECTURE.md`'s data-flow
  section describes for the *timing* functions (`countdown_window`, word
  timing, ...) - it extends to this one small piece of layout math too,
  so the `.cdg` and `.mp4` title cards can never disagree about when
  they end.
- **`project.rs` depends on `video.rs`**, which looks backwards at first
  (a persistence module depending on a *renderer*) - but it's only
  depending on `video.rs`'s small, plain data types (`Background`,
  `BackgroundFit`, `Resolution`, `VideoCodec`), which happen to be defined
  there because that's the module they're most relevant to, not because
  `project.rs` needs any of `video.rs`'s actual rendering logic.
- **`timeline.rs`'s production code has zero crate-internal
  dependencies** - the one `use crate::lyrics::...` in that file is
  inside its own `#[cfg(test)]` module (a regression test building a
  realistic `LyricLine`/`TimedLine` fixture), not something the real
  drag-resolution math depends on.
- **`lyrics.rs`, `cdg.rs`, `ctc.rs`, and `stft.rs` are true leaves** -
  each has zero dependencies on anything else in this crate, which is
  exactly why each is simple to unit-test with synthetic inputs and no
  real model/audio/GUI setup at all.

### How data crosses module boundaries

A handful of types defined in one module are handed, by value or by
reference, across several others - these are the real "connective
tissue" of the app:

- **`lyrics::{LyricLine, TimedLine, Singer, TimingSettings, BackingVocal}`**
  - the most widely shared types in the whole crate. `LyricLine` lives in
  `main.rs`'s `KaraokeApp::lines` and `project::ProjectFile::lines`;
  `TimedLine` is produced by `lyrics::resolve_timing_with_settings` and
  consumed by `export.rs`, `video.rs`, and `main.rs`'s own preview/
  timeline code - never recomputed independently by any of them.
- **Two parallel color representations, never the same struct.**
  `export::Palette` (CD+G's 4-bit-per-channel colors) and
  `video::VideoPalette` (full 8-bit) are each built fresh every frame by
  `main.rs`'s `palette()`/`video_palette()` from the *same* underlying
  `egui::Color32` fields - see the `main.rs` chapter's own gotcha on this
  for the third representation (`project::RgbColor`, for JSON) that
  exists purely for serialization.
- **`project::ProjectFile` is the serialization boundary** between live
  `KaraokeApp` state and disk - `main.rs::to_project_file`/
  `apply_project_file` are the *only* two functions that cross it in
  either direction (see the Data Model chapter).
- **`mdx::MdxModel`/`align::AlignLanguage`** cross from their own modules
  into `main.rs`'s UI (the model/language pickers) and back out into
  `vocals.rs`/`align.rs` as plain arguments - neither carries any state
  of its own beyond which variant was picked.

### The threading model

Three genuinely different kinds of concurrency exist in this app, and
it's worth being able to tell them apart:

1. **Audio playback's own thread, owned entirely by `rodio`.**
   `AudioPlayer` (`audio.rs`) never spawns a thread itself - `rodio`'s
   `Sink`/`OutputStream` do that internally. `main.rs` only ever tracks a
   wall-clock position (`PlaybackClock`) on the *main* thread, deliberately
   decoupled from the real audio device precisely so play/pause/seek math
   can be unit-tested without one.
2. **One-shot background jobs**, for anything slow enough to block a
   frame: waveform decoding, system-font enumeration, auto-align, and
   combined export. Every one of these follows the *exact* same shape
   (first described in the `main.rs` chapter's "background-job pattern"
   section - this is the canonical, cross-cutting version of it):

   ```
   start_*()                          update(), every frame
   ┌─────────────────────┐            ┌──────────────────────────┐
   │ std::thread::spawn(  │            │ poll_*():                 │
   │   move || {           │            │   lock the Mutex           │
   │     ... real work ... │  Arc<Mutex │   .take() the result if   │
   │     *result.lock()    │<-<Option<T>>->  Some -> apply to self   │
   │       .unwrap()        │  (shared)  │   else: still running,    │
   │       = Some(outcome); │            │     return true           │
   │   }                   │            │                           │
   │ );                     │  Arc<Atomic│ if poll_*() returned true:│
   │ progress reported via  │->U32>---->│   ctx.request_repaint()   │
   │ the Atomic as it goes  │ (shared)   │                           │
   └─────────────────────┘            └──────────────────────────┘
   ```

   The owned data each thread needs (file paths, model choices, a
   cloned `Vec` of requests) is *moved* into the closure - never a
   borrow of `self`, since the thread can easily outlive the frame that
   started it.
3. **The `ffmpeg` subprocess**, a genuinely separate *process* (not a
   Rust thread at all), piped to over stdin and read from stdout/stderr
   - `video.rs`'s encode loop and background-video decoder, and
   `vocals.rs`'s `encode_via_ffmpeg`, all go through `ffmpeg_path::command()`
   and standard `std::process` plumbing.

### Keeping the UI in sync with background work

egui is an *immediate-mode* GUI: there's no persistent widget tree to
push updates into - every frame, `update()` runs top to bottom and
redraws everything from the current state. That has a direct consequence
for background jobs: **if nothing tells egui to redraw, a finished job's
result won't show up until the user moves the mouse or types something**
(egui's default repaint trigger is user input, not a timer). This is
exactly what `ctx.request_repaint()` is for, and why every `poll_*`
method's `true` ("still running") return value matters just as much as
its `false`/result-handling path - `update()`'s own housekeeping section
calls `request_repaint()` for precisely as long as a job (or audio
playback itself, which has the same issue) is still active, so a
progress bar actually animates instead of appearing frozen between
unrelated user interactions.

## Deep Dives

Seven end-to-end walkthroughs - following one real user action all the
way through the system, across whatever files it actually touches,
rather than describing one file in isolation. Each assumes you've read
the relevant file-level chapters above; these are about the *sequence*,
not re-explaining what each function does.

### The CD+G format, and how `cdg.rs` encodes it

A `.cdg` file is nothing but a flat sequence of 24-byte packets, played
back by a real CD+G decoder at a fixed, unchanging rate of 300 packets
per second - there is no timestamp field anywhere in the format. This
single fact is why `CdgWriter`'s entire timing model is "how many
packets have been written so far" (`current_time_secs`) and why
`advance_to`/`pad_until` (inserting no-op filler packets) are the *only*
way time moves forward - a renderer that wants something to happen at
3.5 seconds just needs 1,050 packets to already exist before writing the
instruction for it.

The real CD+G spec defines more instruction types than this app ever
uses - notably two *scroll* instructions (shifting the whole visible
canvas by a few pixels, used by some real commercial discs for
scroll-on effects) and an XOR variant of the tile-block instruction
(`INST_TILE_BLOCK_XOR`, defined as a constant in `cdg.rs` but marked
`#[allow(dead_code)]` - documented for completeness, never actually
emitted). This app's own encoder only ever needs five instructions:
memory preset and border preset (clear the screen/border to one
palette color), two CLUT-load instructions (load 8 colors into either
half of the 16-color palette), and plain tile-block draws - because
every single thing this app draws (title card, lyric text, the
countdown dots, the legend) is just text or a filled dot, always drawn
fresh rather than scrolled or inverted. The fixed canvas is 50×18 tiles,
6×12 pixels each (`TILE_COLS`/`TILE_ROWS`); this app keeps to a 48×16
*safe* area to avoid the overscan border some real players/TVs crop,
applying a fixed one-tile offset (`SAFE_ROW_OFFSET`/`SAFE_COL_OFFSET`) to
every coordinate it's given.

`export.rs`'s `render_cdg_with_settings` is the only caller that
actually drives this machinery: it loads both CLUT halves once at the
very start, draws the title card (if any) and advances to when it
should disappear, then for each timed line in order - advance to its
start, clear the relevant rows, draw the line's/preview line's/backing
vocal's base (unsung) text, then advance through every word's own
highlight time, redrawing just that one character's tile in the
highlight color via `draw_char_at`. The color-wipe effect you actually
see on a real player is nothing more than a sequence of single-character
tile redraws, each scheduled at a precise packet position.

### Audio decoding and `ffmpeg` handling, end to end

Two genuinely separate systems handle audio/video I/O in this app, and
it's easy to conflate them:

**Loading/playing the song you're timing against** never touches
`ffmpeg` at all - `audio.rs` decodes directly via `rodio` + `symphonia`
(pure Rust, no subprocess), for both playback (`AudioPlayer::load`/
`play_from_start`/`seek`) and the one-off waveform-peak extraction
(`waveform::build_waveform`, on its own background thread). This is
also what the vocal-separation/forced-alignment pipelines use to decode
their own input audio (`vocals::load_stereo_44100`,
`align::load_mono_16k`) - still no `ffmpeg` involved, since `rodio`/
`symphonia` already read every format this app accepts.

**`ffmpeg` only enters the picture for *encoding*, or for decoding a
background *video*.** Three call sites, all going through
`ffmpeg_path::command()`: `video.rs`'s `render_video` (the real export -
pipes raw RGB24 frames to its stdin, muxes in the loaded audio file, and
encodes with whichever codec was picked), `video.rs`'s
`spawn_background_video_decoder`/`extract_video_background_thumbnail`
(reading a background *video* file - something `rodio`/`symphonia`
don't handle, since they're audio-only), and `vocals.rs`'s
`encode_via_ffmpeg` (converting a separated stem's WAV to MP3 for
non-WAV stem exports). Locating the actual `ffmpeg` binary (and the
`libopenh264` library its H.264 encoder needs) is `ffmpeg_path.rs`'s
entire job - bundled resource, else per-user cache, else (dev builds
only, and only for `ffmpeg` itself) a system install on `PATH`.

### Vocal separation, end to end

Starting from either an export checkbox or auto-align's own isolation
step: `main.rs` calls `vocals::load_separator()` (always `MdxModel::
InstHq3`) or `load_separator_with_model(model)` (auto-align's own
selectable choice), which resolves the actual `.onnx` file via
`model_assets::resolve_model` (bundled → cached → downloaded-and-hash-
verified) and constructs an `MdxSeparator`. `vocals::separate` then
decodes the input file to 44.1kHz stereo (`load_stereo_44100`,
resampling via `rubato` if needed) and hands it to
`MdxSeparator::separate`, which: peak-normalizes the mix (scaling down
only if it exceeds 0.9, never amplifying), chunks it with 25% overlap
(`demix`), and for each chunk - `run_model` converts it to a spectrogram
(`stft::Stft::forward`), zeros the first 3 frequency bins, runs the real
ONNX model, and converts the model's output spectrogram back to audio
(`Stft::inverse`). Chunks are windowed and overlap-added back together,
cropped to the original length, then the model's *primary* output is
rescaled to the original peak and the *secondary* stem is derived by
subtraction from the normalized mix. Whichever of
`Separated.instrumental`/`Separated.vocals` ends up being "the derived
one" depends entirely on which model was loaded (`MdxModel::KimVocal2`
flips it). Finally, `vocals::write_stem_to_file` writes each requested
stem to WAV directly, or to WAV-then-`ffmpeg`-transcode for any other
format.

### Forced alignment, end to end

Clicking "🪄 Auto-align words" calls `main.rs`'s `start_word_alignment`,
which first builds one request per already-timed, *multi-word* line
(skipping single-word lines - nothing to split), each with a padded
search window bounded by its neighbors (`align::WINDOW_PAD_SECS`, and
never crossing into a neighboring line's own window). That work moves to
a background thread: first `vocals::separate_vocals_to_temp_wav` isolates
vocals (falling back to the original mixed audio if separation itself
fails - an accuracy improvement, not a hard requirement), then
`align::Aligner::load` loads the selected language's vocab
(`ctc::Vocab::parse`, embedded `assets/align_vocab/*.json`) and `.onnx`
model (`model_assets::resolve_model` again), and decodes the chosen audio
to mono 16kHz (`load_mono_16k`). For each line's request, `align_line`:
z-score normalizes that window of audio, runs the model to get raw
per-frame logits, converts them to log-probabilities
(`ctc::log_softmax`), builds one flat token sequence for the line's
words (vocab IDs per character, with the vocab's word-delimiter token
between words), calls `ctc::separate_repeats` (so adjacent identical
letters are distinguishable to the trellis), then `ctc::forced_align`
(build the trellis, backtrack it) to get each word's `[start_frame,
end_frame)` span - converted back to absolute song-time via the model's
fixed frame-to-seconds ratio. `poll_align_job` applies every successful
line's word times directly into `self.lines[...].word_overrides`/
`word_end_overrides`, and the line's own `start`/`sing_end_override` are
adjusted to match the first/last aligned word, so the line bubble and
its own outer word bubbles land already in sync on the timeline.

### The lyrics/timeline data model, from a single drag

Dragging a bubble on the fine-tuning timeline is the clearest way to see
every layer of the data model work together in one gesture. On
`drag_started()`, `main.rs`'s `draw_timeline` captures a `TimelineDrag`
(which line, whole-line-or-one-word, and a `timeline::DragSession` -
original start/end, drag bounds derived from the *neighboring* lines,
and the pointer's starting x). Every subsequent frame while the mouse
button is held, `DragSession::resolve(current_pointer_x, px_per_sec)` is
called fresh against that *same* captured origin (never an accumulated
per-frame delta - see the `main.rs` chapter's gotcha on why), returning
new `(start, end)` values clamped by `timeline::DragBounds` so the drag
can never cross into a neighbor's time. `main.rs` writes those values
straight into the real `LyricLine.start`/`sing_end_override` (or, for a
word-level drag, `word_overrides`/`word_end_overrides`) - the *exact*
same fields tapping and the timing table write. Releasing the drag calls
`audition_seek` to rewind playback a couple of seconds for immediate
audible feedback, and `main.rs`'s own `track_undo_history` (running at
the end of every frame regardless of what caused the change) notices the
difference and eventually records one single undo step for the whole
drag, not one per frame. Next time anything needs the *resolved* timing
(the live preview, the timeline's own next redraw, an export),
`lyrics::resolve_timing_with_settings` re-derives a fresh
`Vec<TimedLine>` from the mutated `Vec<LyricLine>` - there's no cached
"resolved" state anywhere to invalidate.

### Project save/load, end to end

"Save Project" calls `main.rs::write_project_to`, which calls
`to_project_file()` (the one function that reads every relevant `self`
field into a `project::ProjectFile`) and then `ProjectFile::save_to_file`
- a plain `serde_json::to_string_pretty` write. "Load Project…" is the
exact reverse: `ProjectFile::load_from_file` (plain `serde_json::
from_str`) into `apply_project_file`, which writes every field back onto
`self` *and* resets everything that's deliberately session-only (timeline
zoom, in-progress text edits, the export dialog's checkboxes, undo
history - see the `main.rs` chapter's own gotcha for why that reset
matters). The crash-recovery autosave is the *same* `ProjectFile`
format, written periodically by `update()`'s housekeeping step
(`project::write_autosave`, to a fixed OS-level path, never the user's
own chosen file) and read back once, at `KaraokeApp::new()`, into
`pending_recovery` - which is what makes the "Recover previous session?"
prompt possible the very next launch if the app never got to clean up
properly last time.

### The export pipelines, end to end

Clicking "Export…" with any combination of outputs checked calls
`main.rs::start_combined_export`, which snapshots everything the
background thread will need (the resolved timing, both palettes, every
export option) and spawns *one* thread covering every selected output,
reporting one combined progress value across equal-weighted phases. Inside
that thread: `.cdg` (`export::render_cdg_with_settings`, then a plain
file write), `.lrc`/UltraStar `.txt` (`formats::export_lrc`/
`export_ultrastar`, same timing), paired audio (`export::copy_paired_audio`,
run once regardless of how many of the three text-ish outputs
triggered it), then - if instrumental audio, vocals audio, or video's
"remove vocals" was requested - vocal separation runs *exactly once*
and every output that needs a stem reuses the same `vocals::Separation`
(see the Vocal Separation deep dive above), and finally `.mp4`
(`video::render_video`, which itself spawns the actual `ffmpeg` encoder
process and, if a background video was chosen, a *second* `ffmpeg`
process purely for decoding it - two separate subprocesses cooperating
within this one phase of this one background thread). `main.rs::
poll_combined_export` applies the final `Vec<ExportOutcome>` to the
status bar once the thread signals completion - each output succeeds or
fails independently, so a missing `ffmpeg` (video export) doesn't stop
the `.cdg`/`.lrc` outputs that don't need it.

## Rust Concepts Used Here

Not a Rust tutorial - a map from language/library features you'll run
into reading this codebase to the exact place each one actually appears,
so an unfamiliar pattern has a concrete real example to check against
rather than an abstract explanation.

- **`Option`/`Result` and `?`** - the backbone of almost every function
  in this crate. `lyrics::parse_timecode` returns `Option<f64>` (`None`
  means "not a valid time," not "zero"); almost everything in the
  Audio/DSP/ML chapter returns `anyhow::Result<T>`, propagated with `?`.
- **`let`-`else`** - used 40+ times across the crate for "bail out early
  if this isn't `Some`/doesn't match," e.g. `vocals.rs::encode_via_ffmpeg`'s
  `let Some(audio) = &self.audio else { return };`-shaped guards. Prefer
  this over a nested `if let Some(x) = y { ... } else { return; }` for
  exactly this "exit immediately otherwise" shape.
- **Enums with associated data and methods, matched exhaustively** -
  `lyrics::Singer`, `lyrics::CountdownMode`, `video::Background`,
  `video::VideoCodec`, `main.rs`'s `PendingUnsavedAction` (`NewProject` vs.
  `LoadProject(PathBuf)` - the second variant actually carries data). Every
  one of these gets matched without a wildcard `_` arm wherever its full
  set of cases matters, so adding a new variant forces the compiler to
  point at every place that needs updating.
- **`anyhow` for application-level error handling** - `.context("...")`/
  `.with_context(|| format!("..."))` to attach a human-readable "what was
  being attempted" message without losing the original error (see almost
  any `Result`-returning function in the Audio/DSP/ML or Data Model
  chapters); `anyhow::bail!`/`anyhow::ensure!` for "fail right here with
  this message" and "fail unless this condition holds," e.g.
  `ctc.rs`/`align.rs`'s `anyhow::ensure!(...)` guards.
- **Traits implemented, not just consumed.** This crate defines no
  custom traits of its own - it implements two from other crates:
  `eframe::App` (`main.rs`'s `update`/`on_exit` - the entire GUI's single
  entry point) and, in tests, nothing further needed. Manual `impl
  Default for X` (as opposed to `#[derive(Default)]`) shows up wherever
  the right default isn't "all fields zeroed" - `CdgWriter::default()`,
  `export::Palette::default()`, `lyrics::TimingSettings::default()`,
  `timeline::View::default()`.
- **`#[derive(...)]`** - `Clone`/`Copy`/`Debug`/`PartialEq`/`Eq`/`Default`
  stacked on small enums and structs throughout (e.g. `codex::Article`'s
  `#[derive(Clone, Copy)]`, added specifically so `all_articles()` could
  build a combined `Vec` - see that chapter). `serde::Serialize`/
  `Deserialize` on everything that round-trips through a project file.
- **`#[serde(default)]` / `#[serde(default = "fn_name")]`** - how
  `project::ProjectFile` stays loadable across every version of itself
  that ever existed: a field added later just needs one of these two
  attributes (see the Data Model chapter's `project.rs` gotchas for why a
  *named* default function is sometimes required instead of the bare
  attribute).
- **Closures as parameters, especially `impl FnMut(f32)`** - every long-
  running operation reports progress through one (`MdxSeparator::separate`'s
  `on_progress`, `Aligner`-driving code in `main.rs`), called from deep
  inside a loop without that loop needing to know *what* the caller does
  with each update.
- **`Arc`/`Mutex`/`AtomicU32` for cross-thread communication** - the
  entire background-job pattern from the "How the Modules Talk" chapter.
  Worth noticing *which* of the two is used for what: `AtomicU32` for a
  single number updated frequently without blocking (progress), `Mutex`
  for a richer value set rarely (the final result) - using a `Mutex` for
  progress too would work, just with unnecessary lock contention on
  every single update.
- **Lifetimes on a struct, not just a function** - `lyrics::TimedWord<'a>`
  borrows `&'a str` for its `text` field rather than owning a `String`,
  since every `TimedWord` is produced fresh from an already-alive
  `TimedLine`/`BackingVocal` and never outlives it.
- **`'static` string slices as compile-time-embedded data** -
  `codex::Article`'s `title`/`category`/`content` fields, and every
  `include_str!`/`include_bytes!` call in the crate (`codex.rs`'s whole
  `ARTICLES` table, `fonts.rs`'s `BUNDLED_FONTS`, `align.rs`'s
  `vocab_json()`, `video.rs`'s embedded DejaVu fonts). Slicing a
  `&'static str` (as `codex::split_markdown_chapters` does) produces more
  `&'static str`s, never a lifetime that needs tracking back to a
  particular call.
- **`OnceLock` for "compute once, cache for the life of the process"** -
  `codex::all_articles()` and `onnxrt.rs`'s own `INIT` (guarding
  `ensure_loaded`'s one-time ONNX Runtime load) are two independent,
  genuine uses of the same pattern, not a coincidence of similar-looking
  code.
- **`std::mem::take`/`std::mem::swap`** - `lyrics::group_into_blocks` and
  `formats::import_kok` both use `std::mem::take(&mut current)` to move a
  `Vec` out of a loop variable while leaving an empty one behind, instead
  of cloning; `video.rs`'s background-video frame handling uses
  `std::mem::swap` to hand a freshly-read buffer over without an extra
  copy.
- **`const fn`** - `cdg::CdgColor::new` and `align::AlignLanguage`'s
  `code()`/`label()` can run at compile time where needed, though nothing
  in this crate currently requires that (regular `fn` would behave
  identically for every real call site today) - it's a cheap, free-to-add
  guarantee for simple, pure constructors/lookups.
- **egui's immediate-mode model** - there's no persistent widget tree or
  event-callback system; `update()` runs top-to-bottom every frame and
  *is* the UI, which is why `ctx.request_repaint()` matters (see "How the
  Modules Talk") and why so much of `main.rs` reads as straight-line
  imperative code rather than a tree of components.

## How-To Recipes

Step-by-step starting points for common changes - naming the exact
files/functions to touch, in the order you'd touch them. Each assumes
you've read the relevant chapters above.

### Add a new auto-align language

1. Get the model's vocab (a HuggingFace-style `vocab.json`) and save it
   to `assets/align_vocab/<code>.json`.
2. Download the model's real `.onnx` file once and compute its SHA-256 -
   this project pins a verified hash for every downloaded model (see the
   Audio/DSP/ML chapter's `model_assets.rs`/`align.rs` sections); don't
   invent or copy a hash from somewhere else.
3. In `src/align.rs`: add a new `AlignLanguage` variant, add it to
   `AlignLanguage::ALL`, and add its arm to `code()`/`label()`/
   `vocab_json()` (an `include_str!` of the file from step 1)/
   `model_filename()`/`model_download_url()`/`model_sha256()` (the hash
   from step 2).
4. Nothing else needs to change - `main.rs`'s language picker already
   iterates `AlignLanguage::ALL` generically.
5. Verify: `cargo test align::tests::every_bundled_vocab_parses_and_has_a_
   blank_and_delimiter` passes automatically (it iterates `ALL`); for a
   real end-to-end check, the `#[ignore]`d `real_model_smoke_test` (see
   "Debug an alignment problem" below).

### Add a new lyric export format

1. In `src/formats.rs`: write an `export_<format>(timed: &[TimedLine],
   title: Option<&str>, artist: Option<&str>) -> String`, following
   `export_lrc`/`export_ultrastar`'s shape (consume `&[TimedLine]`,
   never `Vec<LyricLine>` - export always works from *resolved* timing).
   If the format should also be importable, add `import_<format>` and a
   `detect_format` case too (see `CONTRIBUTING.md`'s own note on not
   guessing an unconfirmed format's grammar).
2. In `src/main.rs`: add an `export_<format>: bool` field to
   `KaraokeApp`, a checkbox in `draw_export_dialog`, and a branch in
   `start_combined_export`'s background thread (write the file, push an
   `ExportOutcome`, include it in the `phase_count` calculation).
3. Add tests in `formats.rs` mirroring the existing LRC/UltraStar ones
   (a basic round trip at minimum).
4. Update `FEATURES.md`'s "Exporting to..." section and
   `CHANGELOG.md`.

### Add a new UI panel or setting

1. Add the field to `KaraokeApp` in `main.rs`.
2. Add the widget - inline in `update()`'s relevant panel closure if it
   belongs with existing content there (most of this app's UI works this
   way - see the `main.rs` chapter's "Why `update` is so long"), or a new
   `draw_*` method only if it's a genuinely self-contained window/dialog.
3. **Decide if it should persist per-project.** If yes: add it to
   `project::ProjectFile`, with `#[serde(default)]` (or a named default
   function if the right default for an old file isn't `Default::default()`
   - see the Data Model chapter's `project.rs` gotchas), and wire it into
   both `to_project_file` and `apply_project_file`. If it's session-only
   (like the export dialog's own checkboxes), it deliberately *shouldn't*
   touch `project.rs` at all - `apply_project_file` resets session-only
   fields to fresh defaults on every load, so don't let a new field
   accidentally bypass that.
4. If it affects timing math specifically, consider whether it belongs in
   `lyrics::TimingSettings` instead of a bare `KaraokeApp` field - that's
   the existing pattern for anything export/preview/`.cdg`/`.mp4` all need
   to agree on.

### Change CD+G colors, layout, or palette behavior

- **Colors/presets**: `export::Palette` and its 5 presets
  (`default`/`high_contrast`/`sunset`/`ocean`/`abyssal`) are the single
  source of truth - `main.rs`'s `ColorPreset::palette()` just calls
  through to one of these. A new preset needs a new `Palette` constructor
  method plus a `main.rs::ColorPreset` variant; it does **not** need any
  change to `video::VideoPalette` - the video renderer's colors are
  derived fresh from the *same* live `egui::Color32` fields every frame
  (see "How the Modules Talk"), never from `export::Palette` directly.
- **Layout**: the fixed row constants at the top of `export.rs`
  (`TITLE_ROW`/`CURRENT_ROW`/`BACKING_ROW`/`PREVIEW_ROW`/`COUNTDOWN_ROW`,
  ...) assume a specific vertical budget within the safe 16-row canvas -
  moving one means checking every other row it might now collide with,
  especially the 2-row-tall title/current bands.
- **Palette budget**: this app already uses 14 of CD+G's 16 color slots
  (see the Rendering & Output chapter's `export.rs` gotchas) - adding a
  new always-visible color needs one of the 2 genuinely free high-CLUT
  slots, or taking over a slot nothing currently uses at the same time.

### Add a new font

- **Bundled** (ships inside the binary, always available): drop the
  `.ttf`/`.otf` under `assets/fonts/<name>/`, add an entry to
  `fonts.rs`'s `BUNDLED_FONTS` (an `include_bytes!` + display name pair),
  and add its license info to `THIRD_PARTY_LICENSES.md`. Check the
  license first - `CONTRIBUTING.md`'s bar is commercial use and
  redistribution both genuinely permitted, the same class of license
  DejaVu/Creepster/Nosifer already use; this project has already turned
  down one otherwise-good-looking font ("Bloodlust") over exactly this.
- **Not bundled** (whatever's installed on the user's own machine):
  nothing to do - `fonts::list_family_names`/`load_family_bytes` already
  enumerate every installed system font generically via `font-kit`.
- Either way: this only ever affects the video export and live preview -
  never the `.cdg` export, which has its own fixed bitmap font
  (`font.rs`) with no room for an arbitrary scalable font.

### Add a new Codex section

- **A short, self-contained doc** (the common case): add a root-level
  `.md` file, then one new `Article { title, category, content:
  include_str!("../YourDoc.md") }` entry to `codex.rs`'s `ARTICLES`
  table. `main.rs`'s `draw_codex` needs no changes - it already iterates
  `codex::all_articles()` generically.
- **Something long enough to want its own in-app chapter navigation**:
  follow this very guide's own pattern instead of adding a single giant
  article - see the Rendering & Output chapter's `codex.rs` section for
  why (`egui_commonmark` has no anchor-link support) and exactly how the
  `"## "`-heading split works.

### Debug an alignment, audio, or export problem

- **Forced alignment producing bad word timing**: first confirm it's not
  the host *line's* own tapped window that's off (auto-align can only
  search within `[line.start - WINDOW_PAD_SECS, line.sing_end +
  WINDOW_PAD_SECS]` - a mistimed line has no good window to search at
  all). For the alignment math itself, `src/align.rs`'s real-audio
  integration test is the right tool:
  `ABYSSAL_CDG_ALIGN_TEST_WAV=<path> cargo test --release
  align::tests::real_model_smoke_test -- --ignored --nocapture` (needs a
  real ~1.2GB model already cached - see `model_assets.rs`). For the
  trellis/backtrack logic specifically, reach for `ctc.rs`'s synthetic-
  emission tests instead - no model or real audio needed to isolate a
  pure alignment-math bug.
- **A real `ffmpeg`/vocal-separation problem**: `ffmpeg_path.rs`'s
  `cargo test --release ffmpeg_path::tests::command_produces_a_working_
  ffmpeg -- --ignored --nocapture` exercises the exact same `command()`
  every real call site uses, against a real bundled/cached `ffmpeg`.
  `vocals.rs`'s `cargo test --release vocals::tests::real_model_smoke_
  test -- --ignored --nocapture` does the same for the separation model
  (synthetic tones, not real music - it only checks the pipeline runs to
  completion without NaNs/silence, not separation *quality*).
- **A `.cdg`/`.mp4` export looking wrong for one specific song**: since
  `main.rs` itself isn't unit-tested (see its own chapter), the fastest
  path is usually reproducing the exact lyrics/timing as a new test in
  the *lower* layer that actually renders it - `export.rs`'s or
  `video.rs`'s own test module, constructing the minimal `LyricLine`s
  that trigger the bad output, rather than trying to debug through the
  whole GUI.
- **Something that only reproduces on a real end-user machine, never in
  CI or on a dev box**: check whether it's actually a packaging/signing
  issue rather than a logic bug first - this exact class of bug hit this
  project twice this session (macOS hardened-runtime library validation,
  Windows MinGW runtime DLLs - see the Audio/DSP/ML chapter's
  `ffmpeg_path.rs` gotchas) and in both cases the application code itself
  was entirely correct.

## Build, Test, CI & Release

**Local build.** `cargo build --release` / `cargo test` / `cargo run
--release` - see `README.md`'s "Building" section for OS package
prerequisites and `CONTRIBUTING.md`'s "Before opening a PR" checklist
(`cargo test`, `cargo clippy --all-targets` with no *new* warnings, a
unit test for anything touched in the pure-logic modules, a `CHANGELOG.md`
entry). If the build fails outright, `BUILD_TROUBLESHOOTING.md`'s first
guess is almost always right: this project's committed `Cargo.lock`
needs Rust 1.88+ (the strictest `rust-version` of any locked dependency),
and an OS-packaged `rustc` is frequently older than that - switch to
`rustup` rather than chasing the error further. `cargo build`/`cargo
test` need none of `ffmpeg`, `openh264`, or any ONNX model file - those
are only resolved at *runtime*, the first time a feature that actually
needs one runs (see the Audio/DSP/ML chapter).

**`build.rs`** runs on every `cargo build`, but only *does* anything on
Windows: embeds the app icon and version/description metadata into the
compiled `.exe`'s own PE resources (see the Rendering & Output chapter -
this is separate from `cargo packager`'s own, installer-only icon
config).

**CI** (`.github/workflows/ci.yml`) runs on every push/PR: one job across
all 3 platforms (`cargo fmt --check`, `cargo clippy --all-targets -D
warnings`, `cargo test`, `cargo build --all-targets`), plus a separate
`cargo audit` job checking `Cargo.lock` against the RustSec advisory
database.

**Release** (`.github/workflows/release.yml`), triggered by pushing a
`v*` tag, is the big one - for each of 4 platform targets: runs
`scripts/build-ffmpeg.sh` (see the Rendering & Output chapter), fetches
Cisco's openh264 runtime binary (HTTPS, SHA-256-verified against a
pinned hash - see the Audio/DSP/ML chapter), fetches the bundled MDX
model and ONNX Runtime (building the latter from source for the one
target with no prebuilt binary, macOS x86_64), `cargo build --release`,
then `cargo packager` to produce the actual installer. A separate
`release` job downloads every platform's artifacts, generates a
`SHA256SUMS.txt` alongside them, and creates the GitHub release. Every
action in both workflows is pinned to a commit SHA rather than a mutable
version tag (a real security-hardening change made this session -
protects against a compromised/retargeted upstream Action tag silently
changing what CI executes).

**Where this is genuinely fragile, and why**: the release workflow is
the one part of this project that can't be fully exercised outside real
CI (no local machine has all 4 target platforms, and some of its steps -
the macOS cross-compile, the Windows MSYS2 build - were written from
each tool's own documented flags rather than run locally first). Several
real, previously-shipped bugs only ever surfaced on a genuine release
build or a real end-user's machine: the macOS arm64 openh264 NEON
assembly failure, the AppImage `linuxdeploy` dependency-discovery issue,
the macOS hardened-runtime library-validation rejection, and the Windows
MinGW-runtime-DLL startup failure (the last two both from this session -
see the Audio/DSP/ML chapter's `ffmpeg_path.rs` gotchas for both). If
something only breaks on a tagged release, start by assuming it's one of
these classes of issue, not a logic bug in the Rust code itself.

## Suggested Reading Order

The order this guide was written in is a reasonable default (it's the
order that let each chapter build on real understanding from the one
before it, not an arbitrary choice):

1. **This guide's own Big Picture/Project Map/Glossary** - the shape of
   the whole thing before any code.
2. **`lyrics.rs`** - the data model everything else is built on; nothing
   else will make sense without it.
3. **`main.rs`** - skim it once, specifically to see how `lyrics.rs`'s
   types actually get used end to end (tapping, the timing table, the
   timeline), even though it's the file you'll come back to piece by
   piece rather than read start to finish in one sitting.
4. **`formats.rs` then `timeline.rs`** - the two direct consumers of
   `lyrics.rs`, each small enough to read in one sitting.
5. **`cdg.rs` → `font.rs` → `export.rs`** - the complete, simpler of the
   two renderers, in the order each builds on the last.
6. **`project.rs`, `recent.rs`, `onboarding.rs`** - the persistence
   layer, now that you know what's actually being persisted.
7. **`stft.rs` → `mdx.rs` → `vocals.rs`** - vocal separation end to end,
   reading the DSP foundation before the algorithm that depends on it.
8. **`ctc.rs` → `align.rs`** - forced alignment, same reasoning.
9. **`model_assets.rs` and `onnxrt.rs`** - can come any time after step
   7, but make more sense once you've seen what actually calls them.
10. **`fonts.rs` → `video.rs`** - the richer renderer, once `export.rs`
    (which it partially depends on) is already familiar.
11. **`ffmpeg_path.rs`** - last among the "real" modules, since it's a
    supporting utility nearly everything in steps 7-10 leans on but none
    of them are *about*.
12. **`codex.rs`, `build.rs`, `scripts/build-ffmpeg.sh`** - tooling and
    meta-concerns, read whenever you need them rather than up front.

If you're arriving to fix one specific kind of bug rather than learn the
whole codebase, it's faster to start from whichever Deep Dive matches
and follow its own file references outward instead of this list.

## Practice Exercises

Roughly increasing difficulty. Each names the real files/functions
involved - working the fan-out yourself is the point, not just reading
the answer here.

1. **(Warm-up)** Add a unit test to `lyrics.rs` for a case not already
   covered: a line where *every* word has a manual `word_overrides`
   entry (so the character-weighted estimate in `resolve_word_timings`
   never actually gets used for a start time). Hint: look at the existing
   `word_timings`-adjacent tests for the fixture shape to copy.
2. **(Warm-up)** Add a new `export::Palette` preset (pick any color
   scheme you like) and wire it into `main.rs::ColorPreset`. Hint: follow
   `Palette::sunset`/`Palette::ocean` exactly - you don't need to touch
   `video.rs` at all (see "Change CD+G colors, layout, or palette
   behavior" above for why).
3. **(Easy)** Add a test to `formats.rs` for an LRC file with a
   multi-tag chorus line (`[00:10.00][01:20.00]Chorus line`) that *also*
   has LRC2 word-level tags inside it - confirm both resulting lines get
   the same word timing. Hint: `import_lrc_handles_multiple_time_tags_
   per_line` and `import_lrc2_populates_word_overrides` are the two
   existing tests to combine.
4. **(Easy-medium)** Add a "Stretch" `video::BackgroundFit` variant that
   distorts the aspect ratio to exactly fill the frame (no cropping, no
   letterboxing). Hint: touches `BackgroundFit`'s enum/`label()`,
   `fit_filter` (a new `ffmpeg` `-vf` scale string with no
   `force_original_aspect_ratio`), and `load_image_background`'s
   `image`-crate equivalent for the still-image case.
5. **(Medium)** Pick any constant in `lyrics.rs` that *isn't* already
   exposed via `TimingSettings` (e.g. `SUNG_LINGER_SECS`,
   `COUNTDOWN_LEAD_SECS`) and make it configurable the same way
   `seconds_per_word`/`min_sing_duration`/`countdown_gap_threshold` are.
   Hint: `TimingSettings`'s struct/`Default` impl, `project::ProjectFile`'s
   `#[serde(default)]` field, and the Timing panel's sliders in `main.rs`
   are the three places to touch, in that order.
6. **(Medium-hard)** Add a brand new `Singer` variant (e.g. a 6th voice
   category). Hint: this one genuinely fans out everywhere - `lyrics.rs`
   (`Singer` itself, `ALL`, `label`), `export.rs` (`Palette`'s new color
   pair, `singer_colors`), `video.rs` (`VideoPalette`'s matching pair,
   its own `singer_colors`), `project.rs` (`ProjectColors`'s new fields,
   with `#[serde(default)]` for old project files), and `main.rs` (new
   `color_*` fields, color pickers, the M/F/D/S-style keyboard shortcut
   if you want one). Doing this exercise is the fastest way to really
   feel how many places a new "always visible" concept touches in this
   codebase.
7. **(Hard)** Add SHA-256 hash-pinning to a brand new hypothetical
   download site, from scratch, mirroring `model_assets::resolve_model`'s
   real pattern exactly (bundled → cached → download-then-verify →
   delete-on-mismatch). Hint: re-read that function's own doc comment
   first, then write the equivalent for a made-up "download a bundled
   SVG icon pack" feature as a design exercise - you don't need to wire
   it into the real UI, just get the resolve/verify logic itself right
   and unit-tested the way `model_assets.rs`'s own tests do (a tiny local
   HTTP server, not a mocking library).

## Ideas for Future Improvements

Only items actually grounded in something seen in the real code or its
own comments - not invented wishlist items.

- **`main.rs::resolved_with_indices` duplicates `lyrics::resolve_timing_
  with_settings`'s sort-and-chain logic** rather than calling it,
  specifically to also track each result's original index into
  `self.lines` (see the `main.rs` chapter's own gotcha). A cleaner long-
  term fix would be giving `lyrics.rs` an index-returning variant of
  `resolve_timing_with_settings` itself, so there's only one real
  implementation of "sort by start, chain to the next start" to keep
  correct.
- **No in-app way to tune the MDX separation model's own parameters**
  (segment size, overlap, denoise) or pick a vocal-removal model other
  than the hardcoded `InstHq3` for the general export checkboxes (see
  `FEATURES.md`'s own "Known limitations") - auto-align already has a
  model picker (`align_vocal_model`); extending that same picker to the
  general "Instrumental audio"/"Vocals audio"/"Remove vocals" checkboxes
  would be a contained, well-scoped change given `vocals::load_separator_
  with_model` already exists and does exactly this for auto-align.
- **AV1 export's `crf=32` "was picked ... but hasn't been tuned against
  a real export yet"** - direct quote from `video.rs`'s own comment on
  `VideoCodec::Av1`'s encode args. A real, currently-open calibration
  task, not a hypothetical one.
- **`.kbp` (KaraokeBuilder Studio) import remains deliberately
  unimplemented** - `formats.rs`'s own module docs and `CONTRIBUTING.md`
  are explicit about why (no confident first-hand grammar knowledge, and
  a guess risks silently wrong timing) and what's needed to add it for
  real (a sample file or spec to verify against).
- **No paid code signing or notarization** (Windows Authenticode, Apple
  Developer ID) - `SECURITY.md`'s own "Release artifact integrity" notes
  this is a cost decision, not an oversight; the `SHA256SUMS.txt`
  published with every release (added this session) proves a download
  matches what was actually built, but can't prove *who* built it the
  way real signing would.
