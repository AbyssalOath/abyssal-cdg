# Abyssal CDG Creator

A desktop app (Rust + egui) that turns an audio file and a block of pasted
lyrics into a real karaoke `.cdg` file - the standard CD+Graphics format
used by karaoke machines and "MP3+G" karaoke software.

## What it does

1. **Load an audio file** (mp3, wav, flac, ogg, m4a, aac) - via **"Load
   Audio…"**, or just drag the file onto the window.
2. **Enter a song title / artist** (optional) - shown as an intro card for
   the first few seconds, like a real karaoke video.
3. **Paste your lyrics** (one line per line of text) and click "Parse
   lyrics" - or **"Load lyrics file…"** (or drag the file onto the window)
   to import an existing `.lrc` (LRC1/LRC2), UltraStar `.txt`, or KOK file
   instead. The format is auto-detected, and `[Verse 1]`/`[Chorus]`-style
   section headers (the kind lyrics sites like Genius add) are stripped
   automatically rather than becoming lines you'd have to time. If the
   file already carries real timing (LRC/UltraStar/KOK all do), imported
   lines land pre-timed - LRC2/UltraStar/KOK word-level tags even populate
   per-word timing directly, and UltraStar's `P1`/`P2`/`P3` duet markers
   map onto this app's Male/Female/Duet voices automatically. You can
   still fine-tune anything afterward the normal way.
4. **Play the song and tap along** - press **Space** (or click the big
   button) the instant a lyric line starts, then press it again the instant
   that line finishes being sung - two taps per line, start then end. Lines
   fill in top to bottom automatically, and the button/status text always
   says which one (start or end, and which line) the next press will set.
   Skipping the end-tap is fine too - it just falls back to an automatic
   estimate until you set it, either later or via the timeline/table below.
   Press **M/F/D/S** at any point to set the current line's voice
   (Male/Female/Duet/Screaming) without reaching for the mouse. Once every
   line has both a start and an end, Space stops tapping and instead
   pauses/resumes playback (or starts it, if the song isn't playing yet) -
   handy for fine-tuning without reaching for the mouse each time.
5. **Missed a line, or the song's fast?** Drag the seek bar (or use the
   ⏪5s/5s⏩ buttons, or the left/right arrow keys) to jump straight back to any
   point in the song and pick up tapping again from there - no need to
   replay from the start.
6. **Watch the live preview** update in real time as the song plays, so you
   can see exactly how the exported file will look *before* you export -
   word-by-word color wipe, verse-style multi-line blocks, countdown dots
   during long instrumental breaks (with the finished lines clearing off
   screen partway through a long one, not sitting there the whole time),
   all rendered live from the same logic used to generate the actual
   exported file.
7. **Assign a voice per line** (Male / Female / Duet / Screaming) for duet
   songs or intense sections - each gets its own color pair so singers can
   tell whose line is whose (or when to belt it) at a glance.
8. **Fine-tune individual words** a few ways:
   - Click "Words" next to any line to open a strip of clickable word
     buttons, and a Start/End toggle above them. In Start mode (the
     default), clicking a word sets when its highlight begins; switch to
     End mode to set when it should stop advancing and freeze instead of
     always running until the next word starts (useful for a held note
     followed by a pause). Green = start set, blue = end set, teal = both.
     Once every line has a start time (step 4), this panel opens
     automatically and **follows along with the song** as it plays. Handy
     for most words, but it can snatch the panel away from (or onto) a
     line right as its very first/last word needs a click - toggle
     **Auto-follow** off in the panel to keep it on one line until you
     move on yourself.
   - Fine-tune visually on the **timeline** at the bottom of the window
     (see below) - a waveform of the loaded audio is drawn behind the
     bubbles, so you can drag one to line up with an actual vocal onset
     instead of guessing. Correcting a value (typing, nudging, or
     dragging) automatically rewinds playback a couple seconds so you can
     immediately hear whether it landed right.
   - Or skip tapping every word by hand entirely: **"🪄 Auto-align
     words"** shells out to a forced-alignment tool to fill in real,
     audio-derived word timing for every already-timed line in one click
     (see "Auto-aligning word timing" below - it's the closest thing this
     app has to a "wow, it just did it" feature, with the caveats that
     come with that).
9. **Customize colors** for every element (background, each voice's
   upcoming/already-sung colors, the next-line preview, and the title
   card) via the color pickers, or pick a built-in preset to skip the
   pickers entirely.
10. **Use an image or video as the background** (album art, a music video,
    ...) instead of a flat color - the **Background** panel below the
    preview lets you choose one, pick a **fit** mode, and adjust a **dim**
    slider (a black scrim) so lyrics stay legible on top of it. By default
    (**Cover**) it scales up to fill the frame and crops the overflow,
    centered - never stretched/distorted, but it can crop off part of an
    image/video that doesn't match the frame's aspect ratio. If that crops
    off something you need to see, switch to **Contain**, which scales the
    whole thing to fit inside the frame instead, adding letterbox/pillarbox
    bars (in the current Background color) rather than cropping anything.
    Shows up in the live preview and the **MP4 video export** (a background
    video is decoded and fit frame-by-frame, looping if it's shorter than
    the song); the live preview shows a single representative frame from a
    video background rather than true playback. The `.cdg` export always
    uses the flat color instead - it's a fixed 300x216, 16-color format
    with no room for real images.
11. **Export** a `.cdg` file, a real **MP4 video**, an **instrumental copy
    of the audio**, and/or **LRC/UltraStar lyrics files** (see below) - any
    combination, from one dialog with one shared progress bar.

Along the way: **Ctrl+Z/Ctrl+Shift+Z** undo/redo almost anything (tapping,
nudging, dragging, auto-align, parsing), so there's no need to be careful
about experimenting; **Save Project**/**Load Project…** (`.abyzl` files, a
"Recent" menu for quick reopening) let you pick up exactly where you left
off, with autosave and a recovery prompt protecting against a crash or an
accidental close.

### Two very different export formats

- **`.cdg`** is the real, legacy CD+Graphics format used by karaoke
  machines and MP3+G/karaoke jukebox software. It has a hard technical
  ceiling: the format is fixed at **300x216 pixels**, always, on every CDG
  file that has ever existed - that's baked into the CD+G subchannel
  standard itself, not a setting. There's no way to make a `.cdg` "1080p"
  any more than you can make a fax machine transmit in 4K. What we *can*
  do within that ceiling: the current line now renders at 2x size (bigger,
  blockier, more legible - closer to real commercial karaoke discs) when
  it's short enough to fit, automatically falling back to normal size for
  longer lines so nothing clips off-screen.
- **`.mp4`** is a real video file (H.264 + AAC, at your choice of 1080p or
  4K) with proper anti-aliased text - this is the "looks like a modern
  KaraFun/karadeo.com video" option, and it has no resolution ceiling at
  all. It's a much heavier file and needs `ffmpeg` installed (see
  Building, below), but it'll look sharp on any screen. Unlike the `.cdg`
  path (which only has room for one current line + a one-line preview),
  the video groups lines into verse-style blocks of up to 5 lines, split
  **exactly wherever you left a blank line in your pasted lyrics** - so if
  you pasted 4 lines, a blank line, then 2 more, you get a 4-line block
  followed by a 2-line block, regardless of how the actual singing timing
  falls. Already-sung lines in a block stay fully colored, upcoming lines
  sit dim underneath, and the active line's color sweeps continuously
  left-to-right through its own letters in real time - a true pixel-level
  wipe, not a word or letter popping to a new color all at once.

Both exports are driven by the exact same lyric timing, word-wipe, duet
colors, countdown, and title card - so however you time and color your
song, it'll come out consistently in both formats.

### The "get ready" countdown

If there's a long instrumental break before a line starts (more than ~5
seconds after the previous line finishes being sung), four dot markers
appear and light up one at a time during the final 4 seconds before the
next line begins - a heads-up so the singer knows to get ready. Each dot
lights up at the start of its own quarter of that window and then stays
lit, so by the time the next line actually starts all 4 have been visibly
lit for a moment rather than the last one flashing on for a single instant
right as the line changes. The same indicator also appears if there's a
long instrumental intro before the very first line (after the title card
fades), so a long intro doesn't just sit on a blank screen with no clue
when the singing starts. This is based on an estimate of how long each
line takes to sing (word count x a rough seconds-per-word figure), unless
you've tapped/set that line's end explicitly (see below).

During a break long enough to trigger the countdown, the already-sung
line(s) also clear off screen partway through instead of sitting there for
the whole gap: they linger briefly after singing ends, then the screen
goes blank until the countdown dots appear, then the next line - rather
than leaving finished lyrics on screen the entire time. Lines still queued
up further ahead in the same verse block are hidden the moment the current
line finishes too, for the same reason - they're still a break away.

### The fine-tuning timeline

Below the lyrics table is a kdenlive/karaodeo-style timeline: one draggable
"bubble" per lyric line, positioned and sized by its start/end time, with a
row of individual word bubbles underneath (shown for every line at once,
not just a selected one). This is a second way to time things - everything
it does, it does by writing the exact same `start`/`sing_end_override`/
per-word fields that tapping and the timing table set, so all three stay in
sync automatically, however you choose to work:

- **Drag a bubble's body** to move it (start and end shift together, same
  duration) - dragging the first or last word of a line past its line's own
  boundary stretches the line's bubble to match, so you're never blocked
  from extending a line just because you started at its edge.
- **Drag an edge** to trim just that side (start or end) independently.
- **Click empty space** (or the time ruler) to jump playback there; **drag**
  empty space to scrub continuously, same as the seek bar up top but right
  on the timeline.
- **Scroll to zoom** (in/out, centered on the cursor); **shift+scroll to
  pan** across a long song.
- The cursor changes to a grab/resize hand depending on whether a drag
  would move or trim a bubble, so it's clear what a click-and-drag will do
  before you commit to it.
- While playing, the view auto-scrolls to keep the playhead in frame
  instead of running off the edge of a zoomed-in window.

### Removing vocals

Two export options produce a vocals-reduced copy of the loaded audio, for
singing/timing against an instrumental instead of the original track
without having to build a second project:

- **"Export instrumental audio…"** saves a standalone `.mp3`/`.wav`.
- **"Remove vocals" checkbox** on the video export mixes the instrumental
  copy into the exported `.mp4` instead of the original audio.

Both run real ML source separation (the UVR-MDX-NET-Inst_HQ_3 model via the
[`audio-separator`](https://github.com/nomadkaraoke/python-audio-separator)
command-line tool) rather than a crude filter trick, but **this requires
`audio-separator` installed separately - it is not bundled with this app**:

```bash
pip install audio-separator          # CPU
pip install "audio-separator[gpu]"   # faster, needs a compatible Nvidia GPU
```

The app checks for `audio-separator` before starting and gives a clear
install message if it's missing, the same way it does for `ffmpeg`. The
separation model file (~100+MB) downloads automatically on first use
(needs internet once) and is cached by `audio-separator` itself for later
runs. Separation is much slower than the app's other exports - anywhere
from several seconds to a few minutes depending on song length and whether
GPU acceleration is available - since it's running an actual neural network
over the audio, not a quick filter pass.

### Auto-aligning word timing

Tapping along sets each line's start/end (two taps), but the word-by-word
wipe *within* that window is otherwise just an estimate (word count x
~0.45s/word) until you fine-tune it. "🪄 Auto-align words" (next to "Reset
all timing") replaces that estimate - and any earlier manual per-word
taps - with real, audio-derived timing for every word in every
already-timed line, in one click.

It works by shelling out to
[`aeneas`](https://github.com/readbeyond/aeneas), a forced-alignment tool,
once per already-timed line - each call is restricted to that line's own
tapped `[start, end)` window (with a little padding), so it only has to
figure out *where within a few seconds of audio* each of that line's own
words falls, rather than aligning an entire song at once. This means
**auto-align builds directly on line-level tapping** - a line with no
timing, or badly mistimed timing, has no window to search and won't get
useful word-level results, so tap along first.

This requires `aeneas` installed separately - it is not bundled with this
app:

```bash
# aeneas also needs eSpeak (or eSpeak NG) and ffmpeg on the system
pip install aeneas
```

See the [aeneas repo](https://github.com/readbeyond/aeneas) for
OS-specific setup notes (eSpeak's Windows situation in particular is a bit
more involved). A run shells out once per timed multi-word line, so it can
take a while for a long song - there's a progress bar, and each line
either succeeds or fails independently (one bad line doesn't stop the
rest). Forced alignment is good, not perfect - review the result and
fine-tune anything that's off the same way you would manually-tapped
timing; Ctrl+Z undoes the whole run in one step if it doesn't help.

**Real-world accuracy caveat:** early testing against a normal mixed
track (vocals + full instrumentation) has been inconsistent - `aeneas` is
a general speech-alignment tool, not something built or trained for
singing over music, and it seems to want a clean, speech-like audio
sample rather than a produced song with a beat and instruments sitting on
top of the vocal. If you try it and the result is spotty, that matches
what we've seen so far too - it's not just you. A more promising setup we
haven't confirmed yet: run it against an *isolated vocal stem* instead of
the full mix. This app already shells out to `audio-separator` elsewhere
(see "Removing vocals" above) to pull an instrumental stem out of a song -
the same tool can extract the *opposite* stem (vocals-only, via its
`--single_stem Vocals` option), which in principle should look a lot more
like the kind of clean, single-voice audio `aeneas` is meant for. Auto-align
doesn't do this automatically yet - it aligns whichever audio file is
currently loaded, as-is, with no separate "alignment source" option. As a
manual workaround today, you could extract a vocals-only file yourself
(e.g. with `audio-separator` directly, or another tool), temporarily
**Load Audio…** that file instead of the full mix, run auto-align, then
switch back to the original mixed audio for playback/export - your
tapped/aligned timing stays on the lines, since it's independent of which
audio file happens to be loaded. If that turns out to noticeably improve
results, automating that swap (extract vocals, align, discard the
temporary file) as part of auto-align itself would be a reasonable next
step.

### Live preview vs. the exported file

The in-app preview isn't a literal decoder for either export format - it's
a second renderer that draws the *same* timing/color/layout data live,
using the same underlying timing math (word highlight times, countdown
windows, title card duration, block grouping) shared with both exporters.

**Note:** the live preview shows the `.mp4` video's layout - multi-line
verse blocks (up to 5 lines, split on blank lines in your pasted lyrics)
with a continuous pixel-level wipe - since that's the richer of the two and
the one most people are timing against. The `.cdg` export is more
constrained (one current line + a single dim preview line underneath, per
the format's tiny 300x216 canvas - see below) and won't look identical to
the preview even though the *timing* is identical. If you're targeting
`.cdg` specifically, the most direct way to check its exact look is to just
export it - a short song encodes quickly - and skim the result.

Same idea applies to a **video background**: the live preview shows one
representative frame from it rather than true playback (full frame-accurate
video decoding inside that small preview isn't worth the extra machinery),
while the actual MP4 export plays the real footage back in sync with the
song.

The exported `.cdg` file contains only the graphics track - CDG never
contains audio itself. To play it back, put an audio file with the **same
base filename** next to it, e.g.:

```
Never Gonna Give You Up.cdg
Never Gonna Give You Up.mp3
```

This "MP3+G" pairing is what karaoke jukebox software (e.g. PCDJ, Karafun
Player, KJ software, most standalone karaoke machines with a USB/SD slot)
expects and will detect automatically.

### Importing existing lyric files

Besides the paste box, "Load lyrics file…" reads:

- **LRC** (`.lrc`) - both plain LRC1 (`[mm:ss.xx]line`) and word-level
  "enhanced" LRC2 (`<mm:ss.xx>` tags mid-line, which map directly onto
  per-word fine-tuning). Metadata tags (`[ar:]`, `[ti:]`, etc) are
  recognized and skipped rather than misread as lyric lines.
- **UltraStar** (`.txt`) - the beat-based note format used by UltraStar/
  USDX/Vocaluxe/Performous. Pitch is read (so the file's structure parses
  correctly) and then discarded, since this app has no pitch display.
  `P1`/`P2`/`P3` duet markers map onto Male/Female/Duet automatically. Only
  a single constant `#BPM:` is supported (rare mid-song BPM-change lines
  aren't handled).
- **KOK** (DEL MP3 Karaoke / KaraWin) - word-level timestamps. This
  format's line-break convention wasn't confirmed from available
  documentation, so lines are grouped heuristically (a pause over ~1.2s, or
  10 words, starts a new line) - the word-level timing itself is solid,
  but re-group the text afterward if the auto-detected line breaks don't
  match the song.

Format is auto-detected from the file content - you don't need to specify
which one it is. **Not supported:** KaraokeBuilder Studio's `.kbp` (a
complex, proprietary, page/syllable-based project format we don't have
confident grammar knowledge of - happy to add real support given a sample
file to verify against, rather than guess and risk silently wrong timing).
"PowerKaraoke" isn't a distinct file format at all - that software's import
is a configurable wizard over generic delimited text, not a fixed grammar,
so there's no single format to target there either.

### Exporting to LRC / UltraStar

The "Export…" dialog can also produce a portable lyrics file alongside (or
instead of) `.cdg`/`.mp4` - useful if you just want to use this app's
tapping/timeline/auto-align workflow to time a song, without needing a
karaoke video or disc at all:

- **Lyrics (.lrc)** - word-level "enhanced" LRC2 by default (one
  `<mm:ss.xx>` tag per word, from the same timing the color-wipe uses),
  with a plain line-level LRC1 option for older/simpler LRC readers.
  Includes `[ti:]`/`[ar:]` tags when your Title/Artist fields are filled
  in.
- **Lyrics (UltraStar .txt)** - beat-based notes at a fixed high notional
  BPM (chosen for round-trip precision, not a claim about the song's real
  tempo), with `P1`-`P4` player markers written wherever the singer
  actually changes (a single-voice song exports as a plain, non-duet
  file). There's no pitch data - this app doesn't track melody - so every
  note gets a constant placeholder pitch; a game that scores pitch
  accuracy will treat the whole song as one fixed note, but the lyrics and
  timing are real.

Both are the direct inverse of their importers above, and both need every
line to actually be timed first (same requirement, and same warning if
it's not, as `.cdg`/`.mp4` export) - a line with no start/end has nothing
to convert into LRC timestamps or UltraStar beats. Like `.cdg`'s "MP3+G"
pairing, the loaded audio is copied alongside either one under the
matching base filename if you have it loaded, ready to drop into a
folder-based player/game library.

## Installing a prebuilt release

Each [GitHub release](../../releases) includes a `.dmg` (macOS), `.msi`/`.exe`
installer (Windows), and `.AppImage`/`.deb` (Linux) - no Rust toolchain or
build step needed. Two things to know before you install one:

- **macOS: "Apple could not verify... is free of malware."** This app isn't
  currently signed with a paid Apple Developer ID or notarized by Apple (that
  program costs $99/year), so Gatekeeper shows this warning on first launch
  for *any* app downloaded outside the App Store that isn't notarized - it's
  not a sign of anything actually wrong with the app. To open it anyway:
  right-click (or Control-click) the app in Finder and choose **Open**, then
  confirm **Open** in the dialog that appears (this only needs to be done
  once); or go to **System Settings -> Privacy & Security**, scroll to the
  blocked-app notice near the bottom, and click **Open Anyway**. If you'd
  rather not click through a warning at all, you can also strip the
  quarantine flag yourself in Terminal:
  ```bash
  xattr -d com.apple.quarantine "/Applications/Abyssal CDG Creator.app"
  ```
- **Windows: "Windows protected your PC" (SmartScreen)** can appear for the
  same reason (no paid code-signing certificate) - click **More info**, then
  **Run anyway**.

Neither warning means the download was tampered with; it's the standard
"nobody paid Apple/Microsoft to vouch for this build" message every
unsigned/unnotarized indie app shows. See
[SECURITY.md](SECURITY.md) if you want to verify what a release actually
does before running it - it's all open source, built by the same CI
workflow that produced the artifact.

## Building

You need a normal, reasonably current Rust toolchain (install via
[rustup](https://rustup.rs) if you don't have one - `rustc --version`
should be 1.75 or newer, ideally current stable).

**For MP4 video export**, you'll also need `ffmpeg` installed and on your
PATH (the `.cdg` export doesn't need it):

```bash
# Debian/Ubuntu
sudo apt install ffmpeg
# macOS
brew install ffmpeg
# Windows: download from https://ffmpeg.org/download.html and add it to PATH
```

The app checks for `ffmpeg` before starting a video export and will tell
you clearly if it's missing, rather than failing silently.

**For vocal removal** ("Export instrumental audio…" and the video export's
"Remove vocals" checkbox), you'll also need
[`audio-separator`](https://github.com/nomadkaraoke/python-audio-separator)
installed and on your PATH - see "Removing vocals" above.

**For auto-aligning word timing** ("🪄 Auto-align words"), you'll also need
[`aeneas`](https://github.com/readbeyond/aeneas) importable by `python3` -
see "Auto-aligning word timing" above.

None of `ffmpeg`, `audio-separator`, or `aeneas` is a *build*-time
dependency (`cargo build`/`cargo test` don't need any of them); they're
only checked at runtime, right before the feature that needs them actually
runs.

The fonts used for video export (DejaVu Sans / DejaVu Sans Bold) are
bundled in `assets/` under the permissive Bitstream Vera license (see
`assets/DEJAVU-LICENSE.txt`) - no extra download needed.

On Linux you'll also need the GTK3 and X11/Wayland development headers that
`rfd` (file dialogs) and `winit` (windowing) build against, plus ALSA's
development headers for `rodio`/`cpal` (audio playback) - most desktops
already have the ALSA *runtime* installed, but the `-dev`/`-devel` package
(with the `alsa.pc` pkg-config file) is a separate install, and its absence
is the most common "why won't this build" surprise on a fresh machine or a
minimal CI container:

```bash
# Debian/Ubuntu
sudo apt install libgtk-3-dev libxkbcommon-dev libx11-dev libasound2-dev pkg-config

# Fedora
sudo dnf install gtk3-devel libxkbcommon-devel libX11-devel alsa-lib-devel

# Arch
sudo pacman -S gtk3 libxkbcommon libx11 alsa-lib
```

macOS and Windows need no extra system packages - just Xcode Command Line
Tools / the MSVC build tools that `cargo` already expects.

Then, from this folder:

```bash
cargo build --release
cargo run --release
```

The first build will take a couple of minutes (it's pulling in a GUI
toolkit and an audio decoder); subsequent builds are fast.

### Running the test suite

The core logic - the CDG packet encoder, the font renderer, and the lyric
timing math - has unit tests that don't need any GUI/audio setup:

```bash
cargo test
```

## How the CDG generation actually works

- `src/cdg.rs` is a from-scratch encoder for the real CD+Graphics packet
  format: 24-byte packets, 300 per second, tile-block graphics commands on
  a 6x12-pixel-tile grid, and a 16-color palette loaded via the CLUT
  instructions. Playback timing is controlled purely by *packet position*
  (no separate timestamp field exists in the format), so the encoder pads
  with no-op filler packets between visible updates to keep everything in
  sync with the audio. 12 of the 16 palette slots are used: the first 8
  (background + Male/Female/Duet's 2 colors each + the next-line preview)
  are completely full, and 4 of the second 8 hold the title/artist card
  plus the "Screaming" voice's color pair - leaving 4 slots free for a
  future addition.
- `src/font.rs` renders text using the `font8x8` crate's bitmap font,
  repacked into CDG's 6x12 tile format.
- `src/lyrics.rs` holds your lyric lines (each with a start time, an
  optional explicit end time, per-word start/end overrides, and a
  `Singer` - Male/Female/Duet) and derives, for each line: per-word
  highlight timestamps (from word lengths and an estimated singing
  duration, unless overridden), whether the leftover time before the next
  line is long enough to warrant a "get ready" countdown, and whether
  already-sung lines should be cleared during a long break. It also owns
  timecode formatting/parsing (`MM:SS.CC`) and the overlap validation
  (start/end can't cross a neighboring line's explicitly-set time) shared
  by tapping, the timeline, and manual entry.
- `src/export.rs` is the actual "karaoke renderer" - it lays out each line
  centered on screen in that line's voice color, draws a dimmed preview of
  the next line underneath, draws the title/artist intro card, re-draws
  each word in the highlight color at its derived timestamp, and draws the
  countdown dots during long breaks.
- `src/audio.rs` probes file duration via `symphonia` and drives playback
  via `rodio`, tracking a wall-clock position for tap-to-time.
- `src/video.rs` is the alternate, unrestricted-resolution renderer: it
  draws each video frame as an RGB24 buffer using `ab_glyph` for real
  anti-aliased TrueType text (the bundled DejaVu fonts), reusing the exact
  same `lyrics.rs` timing/countdown/title-card logic as the CDG path, then
  pipes frames into an `ffmpeg` subprocess (muxed with your loaded audio)
  to produce the final MP4. It runs on a background thread with a progress
  callback so the GUI stays responsive during longer encodes.
- `src/timeline.rs` is the pure time<->pixel math, zoom/drag bounds, and
  drag-mode classification behind the fine-tuning timeline - kept separate
  from the egui widget/painting code in `main.rs` so it's unit-testable
  without a running GUI.
- `src/vocals.rs` shells out to the `audio-separator` CLI to produce an
  instrumental copy of the loaded audio (see "Removing vocals" above).
- `src/align.rs` shells out to `aeneas` once per already-timed line,
  restricted to that line's own tapped window, to fill in real word-level
  timing instead of the character-count estimate (see "Auto-aligning word
  timing" above).
- `src/main.rs` is the GUI: it also has a **live preview** panel that reads
  the same timing data as `export.rs`/`video.rs` to show a real-time
  mockup of what the exported files will look like as the song plays, the
  fine-tuning timeline widget, and color pickers that write directly into
  the exported palette.

## Known limitations / ideas for extending it

- Text wider than 48 characters gets clipped (very long lyric lines) -
  consider auto-splitting long lines during parsing if you hit this.
- Only Latin/basic-ASCII glyphs are available from `font8x8`'s basic set;
  accented characters will render blank. Swap in a different font source in
  `font.rs` if you need broader character coverage.
- Tapping sets both the start and end of each line (two taps), but the
  word-by-word wipe *within* that window is still an *estimate* by default
  (word count x ~0.45s/word) unless you fine-tune specific words via the
  "Words" panel or the timeline's word bubbles, or run "🪄 Auto-align
  words" (see "Auto-aligning word timing" above) to fill it in from real
  audio instead of guessing. The estimate's constants live at the top of
  `src/lyrics.rs` (`SECONDS_PER_WORD`, `MIN_SING_DURATION`) if you want to
  tune the default instead.
- The countdown indicator triggers automatically whenever the estimated
  leftover gap before the next line is at least 5 seconds
  (`COUNTDOWN_GAP_THRESHOLD` in `src/lyrics.rs`); there's no manual override
  if you want it to show up on a shorter gap.
- The timeline's waveform backdrop is drawn from the *loaded audio file*,
  not the exported `.cdg`/`.mp4` - it's purely a visual aid for aligning
  bubbles against actual vocal onsets, not a preview of anything in the
  output files themselves.
- Forced alignment (`aeneas`) is a real speech-alignment tool, not
  something written or trained for singing over music - results against a
  normal full mix have been inconsistent in our own testing so far (see
  the accuracy caveat under "Auto-aligning word timing" above). It can
  also misfire on heavily melismatic/stylized vocals, overlapping voices,
  or a line whose tapped window doesn't actually contain all of its
  words. Treat its result the same as an estimate: worth reviewing, easy
  to fix by hand (or re-tap the line and run it again) where it's off.
- Seeking rebuilds the playback pipeline and fast-forwards (decodes and
  discards audio) to the target position, rather than using the audio
  container's built-in seek tables - this is slightly heavier per seek but
  far more reliable across formats (some MP3 encodings in particular have
  unreliable seek tables). Decoding runs many times faster than real time,
  so a seek anywhere in a typical song is still effectively instant.
- The timing table can be wider than the window at small sizes (long
  lyric lines, plus the singer dropdown and buttons); it scrolls both
  ways rather than overlapping the side panels. Drag the divider between
  the side panels and the middle to resize if you want more room.
- The live preview is a second renderer reading the same timing data, not
  a decoder of the actual `.cdg` bytes - see "Live preview vs. the exported
  file" above.
- Vocal removal always uses one hardcoded model
  (`UVR-MDX-NET-Inst_HQ_3.onnx`) - there's no in-app way to pick a
  different `audio-separator` model or tune its parameters (segment size,
  overlap, etc.) yet.

## More documentation

- [ARCHITECTURE.md](ARCHITECTURE.md) - module layout, data flow, and the
  design decisions behind the two export pipelines.
- [CONTRIBUTING.md](CONTRIBUTING.md) - dev setup, coding conventions, and
  how to propose changes (including new lyric-file format support).
- [SECURITY.md](SECURITY.md) - this app's attack surface and how to report
  a vulnerability.
- [CHANGELOG.md](CHANGELOG.md) - notable changes by version.
- [BUILD_TROUBLESHOOTING.md](BUILD_TROUBLESHOOTING.md) - the most common
  build failure (an old Rust toolchain) and how to fix it.
