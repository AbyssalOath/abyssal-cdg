# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project follows [Semantic Versioning](https://semver.org/) once it reaches 1.0.

## [Unreleased]

### Added

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
- **Automatic word-level timing via forced alignment**: "🪄 Auto-align words" shells
  out to `aeneas` (a separate, non-bundled Python package) once per already-timed
  line, restricted to that line's own tapped window, to fill in real word-level
  timing for every word - instead of tapping each one by hand. Runs on a background
  thread with a progress bar; one bad line doesn't stop the rest, and the whole run
  undoes in one Ctrl+Z. See the README for an important early accuracy caveat
  (results against a full music mix have been inconsistent so far).
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
- **Vocal removal**: "Export instrumental audio…" (standalone `.mp3`/`.wav`) and a
  "Remove vocals" checkbox on video export, both running real ML source separation
  (UVR-MDX-NET-Inst_HQ_3, via the `audio-separator` command-line tool) rather than a
  crude filter. Requires `audio-separator` installed separately - see the README.

### Changed

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
