# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project follows [Semantic Versioning](https://semver.org/) once it reaches 1.0.

## [Unreleased]

### Added

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

### Fixed

- Word-level timeline bubbles could get stuck with zero room to move: a word's drag
  bounds used to be pinned to its immediate neighbor's position, which by default
  touches exactly where the word itself already sits (no gap), leaving nothing to
  drag. Word bubbles are now bounded by the whole line's own window instead.

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
