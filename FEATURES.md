# Features

The full walkthrough of what this app does and how to use it. See
[README.md](README.md) for a quick overview, installing a prebuilt release,
and building from source.

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
   - Click a lyric line (or the "Words" button next to it) to open a strip
     of clickable word buttons for it, and a Start/End toggle above them.
     In Start mode (the default), clicking a word sets when its highlight
     begins; switch to End mode to set when it should stop advancing and
     freeze instead of always running until the next word starts (useful
     for a held note followed by a pause). Green = start set, blue = end
     set, teal = both. The currently-selected line is highlighted in the
     list. Once every line has a start time (step 4), this panel opens
     automatically and **follows along with the song** as it plays, thanks
     to the **"Auto-follow fine-tune panel"** checkbox up in the top
     panel (on by default). Handy for most words, but it can snatch the
     panel away from (or onto) a line right as its very first/last word
     needs a click - turn Auto-follow off to keep the panel on one line
     until you click a different one yourself (or a bubble on the
     timeline).
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
   pickers entirely - the **"Abyssal"** preset also switches the lyric
   font to a bundled spooky display font (**Nosifer**) to match, a
   one-time "set it now" action like the colors themselves (picking a
   different font afterward doesn't get overridden by re-picking the
   preset). A line whose singer is still **Default** (untouched)
   uses its own color pair, separate from Male - it starts out looking
   identical to Male, but can be set independently, e.g. to match a
   background image/video's palette.
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

## Two very different export formats

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
  all. It's a much heavier file and needs `ffmpeg` (bundled with an
  installed copy of the app - see the README's Building section for a dev
  build), but it'll look sharp on any screen. Unlike the `.cdg`
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

## The "get ready" countdown

If there's a long instrumental break before a line starts (more than ~5
seconds after the previous line finishes being sung by default - adjustable
in the **Timing** settings panel, see below) four dot markers appear and
light up one at a time during the final 4 seconds before the next line
begins - a heads-up so the singer knows to get ready. Each dot lights up at
the start of its own quarter of that window and then stays lit, so by the
time the next line actually starts all 4 have been visibly lit for a moment
rather than the last one flashing on for a single instant right as the line
changes. The same indicator also appears if there's a long instrumental
intro before the very first line (after the title card fades), so a long
intro doesn't just sit on a blank screen with no clue when the singing
starts. This is based on an estimate of how long each line takes to sing
(word count x a rough seconds-per-word figure), unless you've tapped/set
that line's end explicitly (see below).

Each line also has its own **Countdown** override in the timing table -
**Auto** (the default, following the gap threshold above), **Force**
(always show the countdown before this line, even on a short gap - it
compresses to fit whatever room is actually there, or shows nothing at all
if there's no room left), or **Suppress** (never show it before this line,
even on a long gap). A ⚠ next to a line set to **Force** means its gap is
short enough that the countdown will be compressed.

The **Timing** settings panel (right sidebar) also lets you adjust the
word-pace estimate itself (seconds per word, and a minimum floor for very
short lines) - these only affect words/lines you haven't fine-tuned by
hand, and are saved with the project.

During a break long enough to trigger the countdown, the already-sung
line(s) also clear off screen partway through instead of sitting there for
the whole gap: they linger briefly after singing ends, then the screen
goes blank until the countdown dots appear, then the next line - rather
than leaving finished lyrics on screen the entire time. Lines still queued
up further ahead in the same verse block are hidden the moment the current
line finishes too, for the same reason - they're still a break away.

## The fine-tuning timeline

Below the lyrics table is a kdenlive/karaodeo-style timeline: one draggable
"bubble" per lyric line, positioned and sized by its start/end time, with a
row of individual word bubbles underneath (shown for every line at once,
not just a selected one). This is a second way to time things - everything
it does, it does by writing the exact same `start`/`sing_end_override`/
per-word fields that tapping and the timing table set, so all three stay in
sync automatically, however you choose to work:

- **Drag a bubble's body** to move it (start and end shift together, same
  duration) - the first/last word's edge and the line's own start/end
  always track each other, in both directions: dragging the first or last
  word past its line's own boundary stretches the line's bubble to match,
  and dragging it back in pulls the line's bubble back with it too, so you
  never have to separately re-adjust the line after fine-tuning its outer
  words.
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

## Removing vocals

Three export options produce a vocals-reduced or vocals-only copy of the
loaded audio, for singing/timing against an instrumental (or reviewing the
isolated vocal) instead of the original track, without having to build a
second project:

- **"Instrumental audio" checkbox** saves a standalone `.mp3`/`.wav` with
  the vocals removed.
- **"Vocals audio" checkbox** saves a standalone `.mp3`/`.wav` of the
  isolated vocals, from the same separation pass.
- **"Remove vocals" checkbox** on the video export mixes the instrumental
  copy into the exported `.mp4` instead of the original audio.

All three run the real UVR-MDX-NET-Inst_HQ_3 ML source separation model
(not a crude filter trick) entirely in-app via [ONNX
Runtime](https://onnxruntime.ai/) - the model ships with the app, so
**there is nothing to install separately**. If more than one of the three
is selected together, separation runs exactly once and both stems (the
instrumental and the vocals) are reused for whichever outputs were asked
for, instead of re-running the model per output. Separation is much slower
than the app's other exports - anywhere from several seconds to a couple
of minutes depending on song length and CPU - since it's running an actual
neural network over the audio, not a quick filter pass.

## Auto-aligning word timing

Tapping along sets each line's start/end (two taps), but the word-by-word
wipe *within* that window is otherwise just an estimate (word count x
~0.45s/word) until you fine-tune it. "🪄 Auto-align words" (next to "Reset
all timing") replaces that estimate - and any earlier manual per-word
taps - with real, audio-derived timing for every word in every
already-timed line, in one click. The line's own start/end move to match
the first/last aligned word too, so the line bubble and its outer word
bubbles land already in sync on the timeline - no separate manual
re-adjustment needed just because the real audio-derived timing came in a
little earlier/later than your original tapped line boundary.

It works by first isolating vocals (the same native separation used by
"Removing vocals" above - **Vocal model** dropdown, next to the button,
lets you pick which one; Inst HQ 3 is the default and generally the best
all-around choice, but Kim Vocal 2 outputs vocals directly instead of
deriving them by subtraction, which *can* align more accurately on a
song where Inst HQ 3's isolated vocal stem isn't clean enough - worth
trying if a particular song's results seem off, at some cost to how
clean the *instrumental* stem would be, which is why it's not the
default. This choice only affects auto-align's own internal vocal
isolation - it has no effect on the "Instrumental audio"/"Vocals audio"
export checkboxes or video export's "Remove vocals", which always use
Inst HQ 3), then running a real speech-recognition model (wav2vec2-CTC,
via [ONNX Runtime](https://onnxruntime.ai/) - see ARCHITECTURE.md)
natively against just the isolated singing, once per already-timed line
- each call is restricted to that line's own tapped `[start, end)`
window (with a little padding), so it only has to figure out *where
within a few seconds of audio* each of that line's own words falls,
rather than aligning an entire song at once. This means **auto-align
builds directly on line-level tapping** - a line with no timing, or
badly mistimed timing, has no window to search and won't get useful
word-level results, so tap along first. If vocal separation itself
fails for any reason, auto-align falls back to aligning against the
original mixed audio rather than failing the whole run - isolating vocals
first is an accuracy improvement, not a hard requirement.

There's nothing to install separately: the language you pick (Language
dropdown, next to the button) downloads its model automatically the first
time you use it (a one-time, roughly 1.2GB download per language, then
cached) - **only the 9 listed languages are supported**, unlike some
general-purpose speech-alignment tools that can work with dozens of
languages via a text-to-speech engine's phoneme output. A run makes one
model call per timed multi-word line, so it can take a while for a long
song, especially the very first line (the model download/load) - there's
a progress bar, and each line either succeeds or fails independently (one
bad line doesn't stop the rest).

**Best-effort, not perfect - always review the result.** This model, like
most speech-recognition models, is trained on clean, speech-only audio -
it wasn't built or trained for singing over a full musical mix. Aligning
against isolated vocals (as described above) substantially closes that
gap versus aligning against the full mix, but it's still a speech model
being asked to track singing, so results can be inconsistent - especially
on heavily melismatic/stylized vocals or overlapping voices. Treat every
auto-aligned line as a fast first pass, not a finished result: play it
back and fine-tune anything that's off the same way you would
manually-tapped timing (the "Words" panel, or the timeline's word
bubbles) before treating the song as done. Ctrl+Z undoes the whole run in
one step if a result isn't an improvement.

## Live preview vs. the exported file

The in-app preview isn't a literal decoder for either export format - it's
a second renderer that draws the *same* timing/color/layout data live,
using the same underlying timing math (word highlight times, countdown
windows, title card duration, block grouping) shared with both exporters.

**Note:** the live preview shows the `.mp4` video's layout - multi-line
verse blocks (up to 5 lines, split on blank lines in your pasted lyrics)
with a continuous pixel-level wipe - since that's the richer of the two and
the one most people are timing against. The `.cdg` export is more
constrained (one current line + a single dim preview line underneath, per
the format's tiny 300x216 canvas) and won't look identical to the preview
even though the *timing* is identical. If you're targeting `.cdg`
specifically, the most direct way to check its exact look is to just
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

## Importing existing lyric files

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

## Exporting to LRC / UltraStar

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
- Forced alignment (the wav2vec2-CTC model) is a real speech-recognition
  model, not something written or trained for singing over music - even
  aligned against isolated vocals (see "Auto-aligning word timing" above),
  results can be inconsistent. It can also misfire on heavily melismatic/
  stylized vocals, overlapping voices, or a line whose tapped window
  doesn't actually contain all of its words. Treat its result the same as
  an estimate: worth reviewing, easy to fix by hand (or re-tap the line and
  run it again) where it's off.
- Seeking rebuilds the playback pipeline and fast-forwards (decodes and
  discards audio) to the target position, rather than using the audio
  container's built-in seek tables - this is slightly heavier per seek but
  far more reliable across formats (some MP3 encodings in particular have
  unreliable seek tables). Decoding runs many times faster than real time,
  so a seek anywhere in a typical song is still effectively instant.
- The timing table can still be wider than the window at small sizes
  (long lyric lines, plus the singer/countdown dropdowns and buttons);
  it scrolls both ways rather than overlapping the side panels, its
  row-selection checkboxes stay visible on the left no matter how far
  right you've scrolled, and the **Compact table** checkbox truncates
  long lyric lines (hover a truncated one for the full text) if you want
  less to scroll through. Drag the divider between the side panels and
  the middle to resize if you want more room - it's remembered across
  restarts.
- The live preview is a second renderer reading the same timing data, not
  a decoder of the actual `.cdg` bytes - see "Live preview vs. the exported
  file" above.
- Vocal removal always uses one hardcoded model
  (`UVR-MDX-NET-Inst_HQ_3.onnx`) - there's no in-app way to pick a
  different separation model or tune its parameters (segment size,
  overlap, etc.) yet.
