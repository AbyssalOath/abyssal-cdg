# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project follows [Semantic Versioning](https://semver.org/) once it reaches 1.0.

## [Unreleased]

### Added

- **Configurable timing settings**: a "Timing" panel (right sidebar) lets you tune the
  word-pace estimate (seconds per word, and a minimum floor for very short lines) and
  the "get ready" countdown's gap threshold, instead of the fixed values every line
  used before. Defaults are exactly those old values, so a project that never opens
  the panel behaves identically to before this existed; saved per-project.
- **Per-line countdown override**: a "Countdown" column in the timing table lets each
  line force the "get ready" indicator on (even for a short gap, clamped to whatever
  room is actually there), suppress it (even for a long gap), or leave it on the
  automatic threshold above - a ⚠ warns when "Force" won't have much room to work
  with. Respected identically by the `.cdg` export, the video export, and the live
  preview.
- **Selectable vocal-isolation model for auto-align**: a "Vocal model" dropdown next
  to auto-align's Language picker lets you try Kim Vocal 2 (outputs vocals directly,
  rather than deriving them by subtraction like the default Inst HQ 3 does) if a
  particular song's alignment results seem off - a real trade-off (cleaner vocals,
  less clean instrumental if it were used for that), not a strict upgrade, which is
  why Inst HQ 3 stays the default. Scoped to auto-align's own internal vocal
  isolation only - the "Instrumental audio"/"Vocals audio" export checkboxes and
  video export's "Remove vocals" always use Inst HQ 3 regardless of this setting.
- **Compact timing table + sticky selection column**: a "Compact table" checkbox
  truncates long lyric lines in the table (hover a truncated one for the full text)
  instead of letting them widen the whole table, and the row-selection checkboxes
  now stay visible in their own fixed column while the rest of the table scrolls
  horizontally, so you can always tell which rows are selected no matter how far
  right you've scrolled.
- **Two more bundled lyric-text fonts**: Creepster and Nosifer (both SIL Open Font
  License 1.1) join the existing system-font picker as selectable options baked
  into the binary - Nosifer is also the "Abyssal" color preset's own default font
  now, applied once alongside its colors the same way the preset's colors are.
- **DISCLAIMER.md**: user responsibility for the audio/lyrics used with this tool,
  kept explicitly separate from the software's own AGPLv3 license (which governs the
  code, not the copyright status of content you process with it).
- **macOS ad-hoc code signing** for release builds (`signing-identity = "-"`) - fixes
  the arm64 build failing to launch at all ("...is damaged and can't be opened").
  Apple Silicon's kernel-level code-signing enforcement (AMFI) requires at least an
  ad-hoc signature just to load an executable, a stricter check than Gatekeeper's own
  "unidentified developer" warning (which the unsigned x86_64 build only ever hit,
  since it happened to run via Rosetta 2 translation, sidestepping the native-arm64
  requirement entirely) - see README's "Installing a prebuilt release" for the
  (unrelated, still-needed) Gatekeeper workaround this doesn't replace.

- **THIRD_PARTY_LICENSES.md**: full attribution and license details for every bundled
  binary/model (ffmpeg, openh264, LAME, ONNX Runtime, the vocal-separation and
  auto-align models) plus a generated license report for the entire Rust dependency
  tree - confirmed no GPL/AGPL-only dependency anywhere in it. See the README's new
  "How this app stays self-contained" section for the bundled-vs-downloaded summary
  this closes out.
- **Backing/echo vocals**: a line can have a second vocalist's phrase
  attached to it (the "Echo" button in the timing table) that overlaps
  part of it while it's still being sung - typically the last word or
  phrase, echoed by a second singer. Tapped independently (its own
  Start/End, bounded within its host line's own window) and colored with
  its own voice, reusing the existing Male/Female/Duet/Screaming palette
  rather than a new color category. The `.cdg` export draws it on its own
  row directly under the current line, with its own concurrent word-wipe;
  the video export draws it directly beneath the current line at a
  smaller size. Deliberately scoped to fall entirely within its host
  line's own timing window (not a fully independent, freely-overlapping
  line) - see `ARCHITECTURE.md`/the code comments on `BackingVocal` for
  why.
- **"Abyssal" color preset**: solid black background, blood-red unsung/dark
  lyric text, warm gold sung/highlight wipe. The red/gold pair is a
  lightness contrast (dark vs. bright), not a hue contrast, so it stays
  distinguishable under deuteranopia, protanopia, and tritanopia alike -
  every other voice pair in this preset (female, duet, screaming) follows
  the same dim-vs-bright principle and avoids a red-vs-green axis entirely.
  Selectable from Colors -> Preset alongside Classic/High Contrast/Sunset/
  Ocean.
- **Custom lyric-text font**: a searchable picker (Colors panel -> Font)
  lists every font family installed on your system (via `font-kit`) and
  applies your choice to the video export and the live preview. Scoped to
  those two renderers only - the `.cdg` export's font is a fixed 6x12-pixel
  1-bit bitmap tile format sourced from a purpose-built bitmap font, not a
  scalable font renderer, so it isn't something an arbitrary system font can
  reasonably substitute into. Saved with the project; opening a project on a
  machine that doesn't have the referenced font installed falls back to the
  bundled default (DejaVu Sans) and shows a notice rather than failing to load.
- **Bulk voice assignment for the timing table**: each line now has a
  selection checkbox (Shift-click for a range, Ctrl/Cmd-click to add/remove
  one at a time, plus "Select all"/"Deselect all") and a "set voice for
  selection" toolbar - select every line, apply the song's majority voice in
  one click, then fix the few that differ with the existing per-line
  dropdown, instead of setting each line individually. Applies as a single
  undoable action.
- **Save/load project files** (`.abyzl`, JSON): captures the loaded audio's path,
  raw lyrics text, every line's timing/overrides, title/artist, colors, and video
  resolution. Ctrl+S saves (to the current file, or "Save Project As…" if there
  isn't one yet); Ctrl+Shift+S always asks for a location.
- **Crash-recovery autosave**: while there's anything worth recovering, the current
  project is written to a fixed slot in the OS's per-user data directory every 30s
  and on every clean exit that still has unsaved changes (including an accidental
  close, not just a crash). If that slot is still there next launch, a "Recover
  previous session?" prompt offers to restore it before anything else is drawn.
- **"Quit without saving?" confirmation**: closing the window with unsaved changes
  intercepts the close and offers Save and Quit / Quit Without Saving / Cancel,
  instead of silently discarding (or silently relying on the autosave) either way.
- **Auto-pair audio on export**: the loaded audio is automatically copied next to
  whichever of `.cdg`/`.lrc`/UltraStar's `.txt` gets exported (same base filename),
  ready for "MP3+G"-style pickup by karaoke players/games without a manual copy or
  rename.
- **Undo/redo**: Ctrl+Z / Ctrl+Shift+Z (or Ctrl+Y) undo/redo lyrics text, timing,
  word overrides, and singer changes - a capped history of up to 100 steps. A whole
  multi-frame gesture (a drag, a text-field edit, a typing session) coalesces into
  one undo step rather than one per frame/keystroke; a whole "Auto-align words" run
  also undoes in one step.
- **Waveform on the timeline**: peaks extracted from the loaded audio (on a
  background thread) are drawn behind the timeline's bubbles, which now have real
  alpha transparency so the waveform stays visible through them - lets you line a
  bubble up against an actual vocal onset instead of positioning by time alone.
- **Drag-and-drop**: dropping an audio, lyrics, or `.abyzl` project file onto the
  window loads it, the same as the corresponding "Load…" button - each recognized
  by extension, with a full-window "drop it here" overlay while a file is being
  dragged over. Multiple files dropped together are each routed independently.
- **Color presets**: a "Preset" dropdown (Classic/High Contrast/Sunset/Ocean) sets
  all 12 colors at once, so a first-time user never has to touch the individual
  color pickers to get a good-looking result.
- **Singer keyboard shortcuts**: M/F/D/S set the voice (Male/Female/Duet/Screaming)
  of whichever line is next up to tap, or, once everything's timed, whichever line
  the fine-tune-words panel is following - no need to reach for each line's dropdown.
- **Recent files menu**: a "🕘 Recent" dropdown lists recently opened/saved project
  and audio files (persisted across restarts), so returning to a previous session
  doesn't mean re-locating the file in a picker.
- **Automatic section-marker stripping**: pasted/imported plain-text lyrics that
  include Genius-style `[Verse 1]`/`[Chorus]`/`[Instrumental Break]`-style headers
  (a line that's *entirely* wrapped in square brackets) have them dropped
  automatically, the same as a blank line - so copy-pasting straight from a lyrics
  site doesn't turn each section header into an extra line you'd have to time.
- **Automatic word-level timing via forced alignment**: "🪄 Auto-align words" runs a
  wav2vec2-CTC speech model natively (via ONNX Runtime) once per already-timed line,
  restricted to that line's own tapped window, to fill in real word-level timing for
  every word - instead of tapping each one by hand. Nothing to install separately;
  covers 9 languages (English, Spanish, French, German, Italian, Portuguese,
  Japanese, Korean, Mandarin), each downloading its own ~1.2GB model on first use and
  caching it after that. Runs on a background thread with a progress bar; one bad
  line doesn't stop the rest, and the whole run undoes in one Ctrl+Z. See the README
  for an important accuracy caveat (results against a full music mix can be
  inconsistent - a vocals-only stem, via the new "Vocals audio" export, works better).
- **Auto-rewind on edit**: correcting a timestamp (typing a new value, nudging, or
  dragging a timeline bubble) automatically seeks playback a couple seconds before
  the edited point, so you can immediately hear whether the correction landed right
  without manually scrubbing back.
- **LRC / UltraStar export**: the "Export…" dialog can now produce a portable
  `.lrc` (word-level LRC2 by default, with a plain-LRC1 option) and/or an UltraStar
  `.txt` note file - the direct inverse of this app's existing importers for both -
  for using the tapping/timeline/auto-align workflow to time a song without needing
  a karaoke video or disc at all.
- **Fine-tuning timeline**: a kdenlive/karaodeo-style widget at the bottom of the
  window showing one draggable/resizable bubble per lyric line (drag the body to move
  it, an edge to trim start/end independently), plus a row of individual word bubbles
  shown for every line at once. Click or drag empty space to seek/scrub playback,
  scroll to zoom, shift+scroll to pan; the view auto-scrolls to keep the playhead in
  frame during playback. Dragging the first/last word of a line past that line's own
  boundary stretches the line's own bubble to match, rather than being blocked.
- **Two-phase tap-to-time**: Space/"Tap next line" now registers a line's start on the
  first press and its end on the second, instead of only ever setting the start - the
  button/status text says which one (and which line) the next press will set.
  Skipping the end-tap is still fine; it falls back to the automatic estimate.
- **Per-word end times**: the "Fine-tune words" panel gained a Start/End toggle -
  clicking a word in End mode sets when its held-out highlight should stop advancing
  and freeze, instead of it always running until the next word starts. Lets a held
  note followed by a pause render accurately instead of stretching to fill the gap.
- **Manual timecode entry**: the timing table's Start/End columns are now editable
  text fields (`MM:SS.CC`, lenient parsing), so a line's timing can be typed directly
  instead of only tapped or dragged.
- **Start/end overlap validation**: a start or end that would cross a neighboring
  line's *explicitly set* time is rejected with a specific error message (e.g. "Start
  (01:05.50) can't be before the previous line ends (01:06.00)") instead of silently
  applied or silently corrupting the ordering - checked wherever a time is set
  (tapping, the per-row "Tap"/"End of line" buttons, and manual entry).
- **Vocal removal**: "Instrumental audio" and "Vocals audio" export checkboxes
  (standalone `.mp3`/`.wav` each) and a "Remove vocals" checkbox on video export, all
  running the real UVR-MDX-NET-Inst_HQ_3 ML source separation model rather than a
  crude filter - natively, via ONNX Runtime, with the model bundled into every
  installer. Nothing to install separately. Selecting more than one of the three
  together runs separation exactly once and reuses both stems, rather than
  re-running the model per output - see the README's "Removing vocals".

### Changed

- **Auto-align now isolates vocals first**: before running the speech-recognition
  model, auto-align separates vocals from the loaded audio (the same native
  separation "Removing vocals" already uses) and aligns against that isolated stem
  instead of the full mix - the CTC model tracks singing far more reliably without
  instrumentation underneath it. Falls back to the original mixed audio if
  separation itself fails, rather than failing the whole run over what's an
  accuracy improvement, not a hard requirement. Previously this required manually
  exporting a vocals-only file and temporarily loading it in place of the song;
  that workaround is gone.
- **The timing table's own start/end always tracks its outer word timing now**:
  auto-align results, dragging a word bubble on the timeline, and tapping a word's
  time in the "Words" panel all keep a line's own start (first word) and end (last
  word) in sync with whichever word bubble actually moved - previously this only
  worked one direction (pushing a word *out* past the line's boundary stretched the
  line to match, but pulling it back *in* left the line stuck at its old, wider
  bound), which meant fixing the last word after an auto-align run often needed a
  second manual adjustment to the line itself.
- **README split into a quick overview + FEATURES.md**: the full feature walkthrough
  (previously the bulk of a ~700-line README) moved to a new FEATURES.md; the README
  itself is now a ~180-line quick overview plus install/build instructions, pointing
  to FEATURES.md/ARCHITECTURE.md for the rest instead of duplicating it.
- The bundled DejaVu Sans/DejaVu Sans Bold fonts moved to `assets/fonts/dejavu/`
  (alongside the new Creepster/Nosifer fonts, each in their own `assets/fonts/`
  subdirectory) - was directly under `assets/`.
- Corrected the documented minimum Rust version to 1.88 (the real floor, from
  `image`/`libloading`'s own declared MSRV in the locked dependency graph) -
  previously documented incorrectly as 1.75, then 1.85, neither of which was ever
  actually checked against the real resolved dependency versions.
- **`ffmpeg` is now bundled, not a system install**: video export and MP3 stem output
  (vocal removal) both used to require `ffmpeg` installed separately on `PATH`; a
  packaged release now bundles a custom-built `ffmpeg` instead (nothing to install for
  an installed copy of the app - `cargo run` dev builds still fall back to a system
  `ffmpeg` if there's no bundled one). This build is `--disable-gpl` (LGPL v2.1-or-later
  only - no libx264/libx265, which would otherwise put the whole app under GPL by
  extension), with H.264 encoding via [openh264](https://github.com/cisco/openh264)
  (BSD) and MP3 encoding via [LAME](https://lame.sourceforge.io) (LGPL; MP3's patents
  expired in 2017). The H.264 encoder specifically doesn't bundle a self-compiled
  openh264 - Cisco's patent-royalty coverage for it only applies to their own
  separately-distributed binary (confirmed directly against openh264's own binary
  license, not assumed), so `ffmpeg` links against openh264 at build time for the
  headers/ABI only, and the actual bundled/downloaded runtime library is Cisco's
  official binary - the same approach Firefox/Chromium use for the same reason. See
  the README's "Building" section and `ffmpeg_path.rs`.
- **ONNX Runtime now loads dynamically on all platforms, including macOS x86_64**:
  vocal removal and auto-align both switched from linking a downloaded ONNX Runtime
  binary at build time to loading one at runtime (`ort`'s `load-dynamic` feature),
  bundled the same way the ML model weights already are. This closes a real gap -
  ONNX Runtime's own prebuilt binaries don't cover macOS x86_64 (Intel) at all
  (dropped upstream between versions 1.23 and 1.25) - by having the release workflow
  build ONNX Runtime from source for that one target (cross-compiled on an Apple
  Silicon runner) and bundling it exactly like the other three targets' downloaded
  binaries, rather than leaving Intel Mac without these two features or without a
  working build at all. A side effect: `cargo build`/`cargo test` no longer make any
  network request at all (previously `download-binaries` fetched a prebuilt ONNX
  Runtime on every build).
- The live preview now shows the same multi-line verse blocks as the video export
  (previously it only ever showed one current line at a time, like the more
  constrained `.cdg` layout) - see the README's "Live preview vs. the exported file"
  for what this does and doesn't mean for `.cdg` accuracy.
- During a long instrumental break, upcoming lines queued later in the same block now
  hide as soon as the current line finishes (instead of sitting there the whole
  break), and already-sung lines linger briefly then clear too, leaving a blank screen
  until the countdown dots appear - applied consistently across the CDG export, the
  video export, and the live preview.
- Manually-timed words in the live preview are no longer shown underlined - the
  distinction wasn't meaningful to a viewer and looked like a rendering glitch;
  precisely-timed words are just held out for their duration like any other word.
- The timing table's columns are now ordered Start / Lyric / End / Singer / ...
- Three separate export buttons/save dialogs (`.cdg`, `.mp4`, instrumental audio)
  are now one combined "Export…" dialog: pick any combination of outputs (now also
  including `.lrc`/UltraStar), one shared folder/base filename, one progress bar.

### Fixed

- **Packaged Linux releases (`.deb`/AppImage) couldn't find any bundled resource at
  all** (ONNX Runtime, the ML models, ffmpeg) - vocal removal and auto-align failed
  outright with a message telling the user to manually download things a real
  packaged release should never need. Root cause: `cargo-packager`'s actual Linux
  layout installs the binary at `usr/bin/<pkg>` and resources at
  `usr/lib/<pkg>/<relative>` (confirmed directly against a real CI packaging log,
  including that the AppImage build reuses the `.deb` build's own `usr/` tree
  wholesale) - the resource-lookup code never checked that exact path, only ones
  missing the `<pkg>` directory segment.
- **AppImage packaging itself failed** ("Could not find dependency: libopenh264.so")
  - `linuxdeploy`'s own dependency-discovery pass resolves a binary's `NEEDED`
    entries via `LD_LIBRARY_PATH`/system paths, not the `$ORIGIN` rpath already baked
    into the built `ffmpeg` (which only matters once the AppImage actually runs) -
    fixed by exporting `LD_LIBRARY_PATH` to include the bundled ffmpeg directory
    before invoking the packager.
- **openh264 (the build-time-only copy `ffmpeg` links against - see `ffmpeg_path.rs`
  for why the real runtime library is separate) failed to build on macOS arm64**
  (NEON assembly toolchain issue) - fixed by disabling its assembly for just that
  one OS/arch combination (it's never shipped or executed, only linked for
  headers/ABI, so this costs nothing). An earlier, broader attempt at the same fix
  disabled assembly for every platform and broke the macOS x86_64 cross-compile
  build instead, due to an asymmetry in openh264's own `Makefile` (only arm64 gets
  its `-arch` flag unconditionally; x86_64's is nested inside the assembly-enabled
  branch) - narrowed to just darwin/arm64 once that was found.
- `scripts/build-ffmpeg.sh`'s `OPENH264_OS` is now auto-detected using openh264's
  own Makefile formula instead of a per-platform guess (which got Windows/MSYS2
  wrong - its `uname` doesn't actually report "msys"), and both LAME's and
  ffmpeg's own `./configure` invocations now `eval` their extra-flags argument so a
  quoted, space-containing value (e.g. `--cc="clang -arch x86_64"` for macOS
  cross-compiling) survives intact instead of being word-split apart.
- **The background image/video preview could fail outright ("Output file is empty,
  nothing was encoded")** for some real-world video files - particularly ones
  downloaded/remuxed via a tool like `yt-dlp`, which can have a leading stretch
  (e.g. a title card or fade-in spliced in without a full re-encode) that fails to
  decode at all under `ffmpeg`'s default "grab frame 0" behavior, even though the
  rest of the file decodes fine. Now tries a cascade of seek points (0.5s, 3s, 8s,
  15s, then literal frame 0) before giving up, and the resulting error (if every
  attempt still fails) includes each attempt's own diagnostic output instead of
  just the last few lines of the final one, which previously cut off the actual
  cause.
- **Dragging a timeline bubble's edge could grab the whole bubble instead,
  if the very first movement was toward the bubble's interior** (e.g.
  dragging left to shrink from the right edge) - the documented-by-users
  workaround was to drag the wrong way first, then back. Root cause: egui
  only recognizes a drag once the pointer has moved past its own
  click-vs-drag threshold (6px as of egui 0.28), so by the time a drag is
  recognized, the reported pointer position has already drifted a few
  pixels in whatever direction you moved first - and that drifted position
  was what decided whether you'd grabbed an edge or the body. Now classified
  from the actual mouse-down position (`press_origin`) instead, which isn't
  affected by that threshold. Applies to both the line-level and word-level
  timeline bubbles.
- **Re-parsing the lyrics text box wiped every line's timing, even lines
  whose text didn't change.** Clicking "Parse lyrics" after fixing a typo,
  adding a line, or reordering lines used to fully discard and rebuild the
  line list from scratch - a serious problem on any song that was already
  tapped/tuned. Re-parsing now diffs the new text against the existing
  lines (matched by content via a longest-common-subsequence pass, not list
  position, so an insertion/deletion/reorder doesn't smear later lines'
  timing) and carries over as much timing as it safely can: unchanged lines
  keep everything; a line reworded with the same word count keeps its
  line-level timing and whichever words didn't change; a line whose word
  count changed keeps its line-level timing but resets word-level timing
  (there's no sound way to line up two different-length word lists); a
  duplicated line (e.g. a repeated chorus) keeps each occurrence's own
  distinct timing rather than risking a swap. Tapping also now correctly
  resumes at the first still-untimed line after a re-parse, instead of
  always restarting from the top.
- **The "get ready" countdown dots could appear after the last lyric line**,
  counting down toward a line that doesn't exist, whenever there was 5+
  seconds of silence after the song's last line finished (e.g. a long
  instrumental outro). The gap-detection logic had no notion of "is there
  actually a next line" - fixed in the `.cdg` export, the video export, and
  the live preview, all of which draw the dots independently. The screen
  still clears/blanks during that trailing silence as before; it just no
  longer shows dots promising a line that never starts. The pre-song
  (intro) countdown is unaffected - it always has a real target (the first
  line) to count into.
- **The background-video/image preview could show a stale letterbox/
  pillarbox color** after changing the background color picker. The actual
  `.mp4` export was always correct (it never caches this), but the in-app
  preview's cached thumbnail only invalidated on the background file or fit
  mode changing, not the color - so the pad color looked "stuck" even
  though a real export already reflected the new color.
- **Windows release installers/executables showed a generic icon instead of
  the app icon.** `cargo packager`'s `icons` config only ever had a single
  large PNG, which is enough for it to auto-generate a macOS `.icns` and to
  use directly for the Linux `.deb`/AppImage icon, but it has no PNG->`.ico`
  conversion of its own and a raw PNG isn't a valid Windows icon resource -
  so the installer/shortcut icon silently fell back to a default, and
  separately, the compiled `.exe` itself never had an icon resource embedded
  at all (that's outside anything `cargo packager` does - it packages an
  already-built binary, it doesn't modify its PE resources). Fixed both: a
  real multi-resolution `.ico` (`assets/icons/abyssal-cdg-icon.ico`) is now
  listed first in `icons` for the installer/shortcut icon, and a new
  `build.rs` (via the `winresource` crate, Windows-only) embeds the same
  icon plus basic version info directly into the compiled `.exe`.
- Space did nothing once every line had both a start and an end (it only ever
  drove tap-to-time, which had nothing left to do) - it now pauses/resumes
  playback instead once tapping is complete (or starts it, if the song isn't
  playing yet), so fine-tuning with the timeline/word panel doesn't require
  reaching for the mouse just to control playback.
- Word-level timeline bubbles could get stuck with zero room to move: a word's drag
  bounds used to be pinned to its immediate neighbor's position, which by default
  touches exactly where the word itself already sits (no gap), leaving nothing to
  drag. Word bubbles are now bounded by the whole line's own window instead.
- Exporting with some (but not all) lines timed used to silently leave the untimed
  ones out of the `.cdg`/video/LRC/UltraStar output with no warning. Export now
  shows a clear message (and disables the Export button) until every line is timed,
  for any output format that actually uses lyric timing.
- The timing table's Start/End fields were slightly too narrow to show a full
  `00:00.00` value without clipping it.

## [0.1.0] - 2026-09-13

Initial release.

### Added

- Load an audio file (mp3, wav, flac, ogg, m4a, aac) and paste or import lyrics.
- Import existing lyric files with auto-detected format: LRC (LRC1 line-level and LRC2
  word-level), UltraStar `.txt` (including `P1`/`P2`/`P3` duet-voice mapping), and KOK.
- Tap-to-time workflow: press Space (or click) to timestamp each lyric line while the
  song plays, with seek-bar scrubbing (drag, ⏪5s/5s⏩, arrow keys) to redo a line
  without replaying from the start.
- Per-word fine-tuning panel that follows playback automatically, for overriding the
  automatic word-highlight estimate on specific words.
- Per-line voice assignment (Male / Female / Duet / Screaming) with independent
  unsung/highlight color pairs for duet songs and intense sections.
- Live preview panel that mirrors the exported file's timing/color/layout live, as the
  song plays.
- Fully customizable colors (background, per-voice pairs, next-line preview, title
  card) via color pickers.
- `.cdg` export: a real, spec-conformant CD+Graphics file (24-byte packets, 300/sec,
  6x12 tile graphics, 16-color palette), with 2x-scaled text for the current line when
  it fits, and a "get ready" countdown indicator during long instrumental breaks
  (mid-song or before the first line).
- `.mp4` video export (H.264 + AAC, 1080p or 4K) via an `ffmpeg` subprocess, with
  anti-aliased text, multi-line verse blocks, a continuous pixel-level color wipe, and
  the same title card/countdown/duet-color logic as the `.cdg` export.

### Fixed

- The intro "get ready" countdown (before the first lyric line, for a long
  instrumental intro) no longer silently fails to appear in the `.cdg` export and the
  live preview when no song title/artist is entered - it was only ever gated on a
  title card being shown, which didn't match the documented behavior or the `.mp4`
  export. The threshold check now lives in one shared function
  (`lyrics::countdown_window_between`) used by all three renderers, so they can't drift
  apart again.
