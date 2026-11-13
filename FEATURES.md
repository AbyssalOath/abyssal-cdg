# Features

The full walkthrough of what this app does and how to use it. See
[README.md](README.md) for a quick overview,
[docs/INSTALLING.md](docs/INSTALLING.md) for installing, and
[docs/BUILDING.md](docs/BUILDING.md) for building from source.

The very first time you launch the app on a given machine, a one-time
disclaimer window (the same content as the Codex's "Legal Disclaimer &
User Responsibility" article - see [DISCLAIMER.md](DISCLAIMER.md)) shows
before anything else, with an "I Understand" button to dismiss it. It's a
per-machine, not per-project, acknowledgment - once dismissed, it never
shows again on that machine regardless of which project you open or
create afterward.

## Contents

- [Finding your way around](#finding-your-way-around)
- [What it does](#what-it-does) - the whole workflow, step by step
- [Two very different export formats](#two-very-different-export-formats)
- [The "get ready" countdown](#the-get-ready-countdown)
- [The fine-tuning timeline](#the-fine-tuning-timeline)
- [Removing vocals](#removing-vocals)
- [Auto-aligning word timing](#auto-aligning-word-timing)
- [Live preview vs. the exported file](#live-preview-vs-the-exported-file)
- [Importing existing lyric files](#importing-existing-lyric-files)
- [Exporting to LRC / UltraStar](#exporting-to-lrc--ultrastar)
- [Copying a chorus's timing](#copying-a-choruss-timing)
- [Syllables (per-syllable timing)](#syllables-per-syllable-timing)
- [Video export options](#video-export-options)
- [Exporting subtitles (.ass / .srt)](#exporting-subtitles-ass--srt)
- [Outro card](#outro-card)
- [Batch export](#batch-export)
- [Pre-export checklist](#pre-export-checklist)
- [Known limitations / ideas for extending it](#known-limitations--ideas-for-extending-it)

## Finding your way around

### The step bar
The step bar across the top of the window -
**Audio › Lyrics › Tap › Fine-tune › Export** - ticks each step off (✔)
as you finish it, highlights the one you're on (with a running "12/40"
count while tapping), and the line under it always says what to do now,
with a button that does it: *Load audio…*, *Parse lyrics*, *▶ Start
tapping* (picks up a couple of seconds before your last tap), *Export…*.
Fine-tuning words is marked ◇ optional - it never stands between you and
exporting. Hover any step for a one-line reminder of what it involves.

### Play controls
One row: **⏮** back to the start, one **▶ Play / ⏸
Pause** button, ⏪ 5s, the seek bar, 5s ⏩, and the time readout - click
it, type a time (`1:23.5`, `1:02:03`, or plain seconds) and press Enter
to jump there. Speed and Loop sit together on the row below.

### The timing table
Each row has a status dot - amber = not
timed yet, grey = timed with estimated word timing, green = fine-tuned
(or a one-word line) - and a progress bar above the table counts them
("23/41 timed · 6 estimated · 17 fine-tuned"). A row's Tap / Words /
Backing / ±0.1s / ✕ buttons appear when you hover it (and stay on the
line you're tapping, the one open in the fine-tune panel, and any
checked rows), so a long song isn't a wall of buttons.

### Keyboard shortcuts
Press **F1** or **?** (or click **⌨ Shortcuts**
in the top panel) for the full list. Beyond tapping (Space, Shift+Space,
M/F/D/S/B): **, / .** nudge the current line's start by 0.1s
(**Shift+, / Shift+.** shift the selected lines instead), **Enter**
re-taps the current line, **Delete** clears its timing, **Home** jumps to
the start, **L** loops the line, **Ctrl+E** opens Export. "Current line"
is the one open in the fine-tune panel, otherwise the one you tapped
last. Single-key shortcuts never fire while you're typing in a text box.

### Messages and toasts
The status line at the bottom of the top
panel is color-coded - green ✔ for success, amber ⚠ for something to do
first (or a partial success), red ✖ for an error - and anything other
than routine progress also pops up as a toast in the bottom-right corner
that fades after a few seconds (click to dismiss). After an export, a
**📂 Open folder** button opens the output folder in your file manager.

### Tips, UI size and remembered settings
Untick **💡 Tips** (top panel) and the
explanations under each control shrink to a one-line summary - hover its
ⓘ for the full text. It's remembered on this computer, and starts on for
a fresh install so first-time users get the full guidance.

**UI size − 100% +** in the top panel (or
Ctrl+= / Ctrl+- / Ctrl+0) scales the whole window - handy on a 4K
screen - and is remembered on this computer. The app's purple accent marks
the main actions (Play, Tap, Export, the step bar's current step) and
selections throughout.

The folder you pick in the export
dialog is filled in automatically next time - even after restarting the
app - as long as it still exists, and **Choose folder…** opens there.

## What it does

1. **Load an audio file** (mp3, wav, flac, ogg, m4a, aac) - via **"Load
   Audio…"**, or just drag the file onto the window.
2. **Enter a song title / artist** (optional) - shown as an intro card for
   the first few seconds, like a real karaoke video. In the video export
   and live preview, a small "Created with Abyssal CDG Creator" credit
   also appears under the title card by default - it's a toggle in the
   export dialog (**Video (.mp4)** options), on by default but never
   forced: uncheck it and it's gone from that export entirely, no
   watermark you can't turn off.
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
   For lines sung straight into each other with no gap, **Shift+Space**
   (or the **End this line + start the next** button under the tap
   button) ends the current line and starts the next one at the same
   instant, in one tap. A live tap is never thrown away: if you've
   seeked back and tap a line's start before the previous line's end,
   that end is pulled back to meet it, and an end tapped after the next
   line's (already-set) start is clamped to it.
   **Tap latency** (under the tap button) subtracts a fixed number of
   milliseconds from every tap - tap-along, re-taps, backing lines, and
   word/line-end taps in the fine-tune panel - to cancel out how late you
   tap after hearing a line start (typically 100-200 ms, more with
   Bluetooth headphones). If your taps consistently land a little late on
   the timeline, raise it. It's saved on this computer, not in the
   project, since it's about you and your headphones rather than the
   song, and it only affects new taps. To fix a song you've already
   tapped, select its lines (or **Select all**) and use **Shift
   selection ◀ Earlier / Later ▶** above the timing table - it moves each
   selected line's start, end and word timing together by the amount you
   set, and refuses (changing nothing) if that would overlap an
   unselected line or go before the start of the song.
   Press **M/F/D/S/B** at any point to set the current line's voice
   (Male/Female/Duet/Screaming/Backing) without reaching for the mouse.
   Tap-along skips backing-vocal lines (see step 7) - they're timed
   separately. Once every
   line has both a start and an end, Space stops tapping and instead
   pauses/resumes playback (or starts it, if the song isn't playing yet) -
   handy for fine-tuning without reaching for the mouse each time.
5. **Missed a line, or the song's fast?** Drag the seek bar (or use the
   ⏪5s/5s⏩ buttons, or the left/right arrow keys) to jump straight back to any
   point in the song and pick up tapping again from there - no need to
   replay from the start.
   **Fast song?** The **Speed** buttons under the play controls (0.5× / 0.75× / 1×)
   slow playback down for tapping fast lines or fine-tuning words. Taps
   still record the real song time (and tap latency compensation scales
   with the speed, since your reaction time is real time, not song time).
   Pitch drops along with the speed - it's a timing aid, not a
   pitch-preserving time stretch - and it only affects playback in the
   app, never anything exported. It stays set across seeks and restarts
   until you change it.
6. **Watch the live preview** update in real time as the song plays, so you
   can see exactly how the exported file will look *before* you export -
   word-by-word color wipe, verse-style multi-line blocks, countdown dots
   during long instrumental breaks (with the finished lines clearing off
   screen partway through a long one, not sitting there the whole time),
   all rendered live from the same logic used to generate the actual
   exported file.
7. **Assign a voice per line** (Male / Female / Duet / Screaming / Backing) for duet
   songs or intense sections - each gets its own color pair so singers can
   tell whose line is whose (or when to belt it) at a glance. For a song
   with more than one voice in the same category (two different men, three
   featured singers, ...), type a **custom name** for a line ("Male 1",
   "Lead Singer", whatever) right next to the voice dropdown - it shows up
   in the legend/dropdown instead of the generic "Male", and once typed
   once, becomes a one-click option in every other line's dropdown too, no
   retyping. A custom name always shares its base category's color (Male 1
   and Male 2 both render in Male's color) rather than getting its own -
   there's no separate color budget for arbitrarily many distinct singers,
   especially in the `.cdg` export's fixed 16-color palette - it's a
   distinct *label*, not a distinct color.

   **Overlapping backing vocals / two-lane duets.** When a second voice
   sings *different* lyrics at the same time as the lead - a backing
   vocalist screaming behind the main line, or a duet where both singers
   overlap with their own words - put that part on its own line in the
   pasted lyrics (typically right under the main line it goes with; if
   your pasted lyrics are missing it, just type it in and click
   **Parse lyrics** again - existing timing is kept and the new line lands
   where you typed it). Then click that line's **Backing** button in the
   timing table. A backing line:
   - is timed completely independently of the main lines, so it can start
     mid-line and run past the end of the line it goes with, into the next
     one (or several);
   - is skipped by tap-along, and gets its own **Tap start / Tap end**
     button instead - easiest on a second play-through once the main lines
     are tapped (or type its times, or drag its bubble on the timeline,
     where backing lines get their own lane);
   - gets the dedicated **Backing** voice color by default (customizable in
     Colors, like every other voice), though any voice/custom name works;
   - shows in its own lane below the main lyrics in the preview and video,
     and on its own row under the current line in the `.cdg` - appearing a
     moment before it starts, with its own word-wipe running alongside the
     main line's.

   Click **Backing** again to put a line back on the main track. Only one
   backing line shows at a time - if two backing lines overlap each other,
   the later one takes over the lane when it starts. LRC export includes
   backing lines interleaved by time; UltraStar export includes main lines
   only.
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
   - **Loop a line while you fine-tune it**: **🔁 Loop this line** in the
     fine-tune panel (or **Loop line** next to Speed, or the **L** key)
     repeats that line with a second of lead-in and half a second after,
     so you can click its words over and over until they land. The loop
     follows the line's timing as you change it, and Auto-follow holds
     still on the looped line meanwhile. For any other stretch of the
     song, **A** and **B** set a free loop from the current playback
     position. The loop only jumps back when playback reaches its end, so
     you can still seek anywhere; **✖** (or L again, for a line loop)
     stops it. The active loop is shaded on the timeline. Pairs well with
     the **Speed** buttons.
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
   **Readability checks.** The Colors section checks your colors the way
   the `.cdg` will actually show them (only 16 shades per color channel,
   so two picked colors can come out identical) for the voices your song
   uses: an upcoming/sung pair too close for the wipe to show, text too
   faint against the background (under the usual 3:1 contrast for large
   text), or two voices singing in nearly the same color. Any problems
   show as **⚠ N** on the Colors header, listed inside it, and as notes in
   the export checklist. Every built-in preset passes (Abyssal's upcoming
   colors were brightened just enough - same hues, still dark and moody -
   to read on a TV across a room); the Classic look now has Duet sing in sky blue rather than orange, which
   is hard to tell from Male's yellow with red-green color blindness.
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
11. **Export** a `.cdg` file (optionally packaged with an `.mp3` as an
    MP3+G `.zip`), a real **MP4 video**, an **instrumental or vocals-only
    copy of the audio**, **`.ass`/`.srt` subtitles**, and/or
    **LRC/UltraStar lyrics files** (see below) - any combination, from one
    dialog with one shared progress bar, after a quick pre-export
    checklist. Several saved projects can be exported in one go with
    **Batch export**.

Along the way: **Ctrl+Z/Ctrl+Shift+Z** undo/redo almost anything (tapping,
nudging, dragging, auto-align, parsing), so there's no need to be careful
about experimenting; **Save Project**/**Load Project…** (`.abyzl` files, a
"Recent" menu for quick reopening, drag-and-drop onto the window) let you
pick up exactly where you left off, with autosave and a recovery prompt
protecting against a crash or an accidental close. Loading a different
project while the current one has unsaved changes - via the file picker,
the Recent menu, or drag-and-drop - prompts first ("Save and Load" / "Load
Without Saving" / "Cancel"), the same protection an accidental window
close already has; the export dialog's own "what to export, and how"
choices (formats, resolution/codec aside - those *are* saved per-project)
also reset to their defaults on every project load, rather than silently
carrying over whatever the previous project had selected.

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
- **`.mp4`** is a real video file (AV1 by default, H.264 selectable for
  compatibility - see "Known limitations" below - plus AAC audio, at
  720p, 1080p or 4K, in 16:9 or vertical 9:16 - see "Video export
  options" below) with proper anti-aliased text - this is the "looks like a modern
  KaraFun/karadeo.com video" option, and it has no resolution ceiling at
  all. It's a much heavier file and needs `ffmpeg` (bundled with an
  installed copy of the app - see [docs/BUILDING.md](docs/BUILDING.md) for a
  dev build), but it'll look sharp on any screen. Unlike the `.cdg`
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

In the video export and live preview (not the `.cdg` export, whose fixed
16-color, 300x216 display has no reasonable way to do a smooth per-line
fade), already-sung lines don't just blank out once a break runs long
enough - each one fades out on its own, top to bottom, a couple of
seconds after *it* finishes, rather than the whole block disappearing at
once on a single collective timer. Once the countdown itself starts, the
next lines fade back in the same way - dimmed, the same look they always
have before singing reaches them - so there's time to read ahead during
the countdown instead of the words only appearing right as they go live,
closer to the smooth, rolling feel of a KaraFun-style display than a
hard slideshow-style cut between blocks. The dots themselves sit near the
top of the frame there too, specifically so they don't compete for space
with the lines they're counting down to.

That countdown-and-reveal handles a *real* break - long enough that a
singer genuinely needs the heads-up. For the much more common case of
one verse block simply running into the next with barely a breath
between them, there's a separate, lighter-weight hand-off: as soon as a
line's own fade-out finishes (freeing up its row), whatever comes right
after the current block rolls into that empty row immediately, block
boundary or not - so the last couple of lines of one verse and the first
couple of the next can be on screen together, rolling up top to bottom,
instead of the whole next block waiting to appear until the line
currently playing actually reaches it. This only happens across a gap
short enough that a countdown wouldn't trigger for it anyway; a real
countdown-worthy gap is left entirely to the mechanism above, not mixed
with this one.

The `.cdg` export keeps its simpler, older behavior instead: already-sung
lines linger briefly after singing ends, then the screen goes blank until
the countdown dots appear, and upcoming lines stay hidden until the line
they belong to actually starts.

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

The preview has two modes, switched with the **Video / .cdg (exact)**
buttons above it (in the side panel or the popped-out window).

**.cdg (exact)** *is* a literal decoder: it renders the real `.cdg` bytes
the exporter would write (in the background, a moment after you change
timing, colors or text) and plays them back at the current position,
exactly as a karaoke player would show them - the 300x216 canvas, its
16-color palette, the 2x/1x text sizing, the one-line-plus-preview
layout, and any clipping. Pixels are scaled up without smoothing so you
see the real blocky look.

**Video** (the default) isn't a literal decoder - it's
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
specifically, switch the preview to **.cdg (exact)** to see its real
look.

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

**MP3+G `.zip`.** Tick **Also package as an MP3+G .zip** under `.cdg` in
the export dialog to also get `Song.zip` holding `Song.cdg` and
`Song.mp3` - the format many karaoke hosts and KJ programs import songs
in. Pick the zip's audio: the **original song** (copied as-is if it's
already an MP3, otherwise converted to a 320kbps MP3 with the bundled
ffmpeg) or the **instrumental** (vocals removed - most karaoke tracks are
instrumentals; this runs the vocal-removal model, shared with any other
vocal-removal export picked at the same time). The loose `.cdg` and
paired audio are still saved alongside it.

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

## Copying a chorus's timing

Choruses usually repeat to the same rhythm, so once one is timed (and its
words fine-tuned), the others don't need tapping from scratch. Tick the
timed chorus lines' checkboxes in the timing table and click **📋 Copy
timing to…**:

- The dialog points at the next place those lines repeat (same words,
  ignoring capitalization and punctuation) - **Find repeat** jumps to it
  again, or set the first line yourself.
- A preview pairs each copied line with its target: ✔ = same words, so
  every word's timing is copied; ≠ = different words, so only the line's
  start/end are copied and its words go back to the estimate.
- Set where the repeat starts: **Where line N already starts** (if it's
  tapped), or play the song and click **⏱ It starts now** the moment the
  repeat begins (tap latency compensation applies).

Every line and word lands at the same spacing as the original. It's
all-or-nothing - if the copy would overlap a neighboring line, nothing
changes and the status says which line - and Ctrl+Z undoes it.

## Syllables (per-syllable timing)

For a word held across several notes, mark its syllables with `|` in the
pasted lyrics - `beau|ti|ful` - and each syllable becomes its own timing
unit: its own button in the fine-tune panel (shown as `beau-` `-ti-`
`-ful`), its own bubble on the timeline, its own highlight time in the
wipe. It still *displays* as one word ("beautiful") everywhere - preview,
video, `.cdg`, subtitles. Adding marks to an already-timed word keeps its
start (now the first syllable's) and end (the last syllable's); the
syllables in between start out estimated.

`|` rather than `-` so real hyphenated words ("well-known", "x-ray") stay
as they are. Auto-align still aligns whole words and divides each word's
time among its syllables by letter count. Exports carry syllables through:
LRC2 writes a tag per syllable (`<..>beau<..>ti<..>ful`), UltraStar a note
per syllable, `.ass` a `\kf` per syllable. Importing works the other way:
UltraStar syllable notes and LRC2 tags inside a word become `|`-marked
syllables with their own times (previously UltraStar syllables were merged
into whole words, and LRC2 mid-word tags made the line's word timing
unusable); UltraStar `~` hold notes extend the previous syllable.

## Video export options

Under **Video (.mp4)** in the export dialog, alongside resolution and
codec (all saved with the project):

- **Resolution** - 720p (quick drafts, smaller files), 1080p or 4K.
- **Shape** - **16:9** (YouTube, TVs, karaoke screens) or **9:16**
  (YouTube Shorts, TikTok, Instagram Reels). Text is sized from the
  frame's short side, so lyrics are the same size either way up instead of
  overflowing the narrow vertical frame. The live preview switches shape
  to match.
- **Lyric size** (60-160%) - the lyric lines and backing lane; the title
  card keeps its size. Long lines still shrink to fit. Shown in the live
  preview.
- **Text outline** / **Drop shadow** - a dark outline and/or soft shadow
  under all text, for legibility over a busy image or video background.
  Exported video only (the live preview doesn't draw them).
- **Fade out audio at the end** - over the last 0.5-15 seconds.

## Exporting subtitles (.ass / .srt)

Two subtitle exports in the export dialog, for putting lyrics on your own
video or uploading captions:

- **Karaoke subtitles (.ass)** - each line appears a second before it's
  sung and each word sweeps from its unsung to its sung color as it's
  sung (`\kf` karaoke tags, from the same word timing as the `.cdg`/video
  wipe - a word with a manually set end pauses before the next one). One
  style per voice in your Colors-panel colors, a smaller style for
  backing lines, and the title/artist card. Uses your chosen lyric font
  (Arial if none is chosen; a player without that font substitutes its
  own). Plays in VLC and mpv, opens in Aegisub, and most video editors can
  burn it into a video.
- **Plain subtitles (.srt)** - one cue per lyric line (backing lines
  included, interleaved by time) plus the title/artist, no word timing.
  The most widely supported format - YouTube captions, any player.

Both need every line timed, like the other lyric exports.

## Outro card

Tick **Outro card** under the song title/artist fields to end the song
with a closing card - your own text (up to 3 lines, the first shown
largest; "Thanks for singing!" by default), optionally followed by the
title and "by artist". It appears 1.5 seconds after the last line (main
or backing) finishes and stays until the song ends, fading in - in the
video, the live preview (both modes), the `.cdg` and the `.ass`/`.srt`
subtitles. The line under the checkbox shows exactly when it'll be on
screen; if the song ends less than 3.5 seconds after its last line,
there's no room and it's skipped. Saved with the project; off by default.

## Batch export

At the bottom of the export dialog, **Batch export** exports several
saved projects (`.abyzl`) in one go, with the dialog's current choices
(which formats, zip audio, LRC style, stem format, remove vocals) into
the chosen folder. **Choose projects…** adds files (pick as many as you
like, run it again to add more, ✖ removes one), then **Export N
project(s)**. Projects are read straight from their files - the project
you have open isn't touched.

- Each song uses its own saved colors, video resolution/codec/shape/style,
  lyric font, background and audio file.
- Files are named the way the export dialog would name them: after the
  song's audio file, else its title. Two songs that would get the same
  name become `Name` and `Name (2)` rather than overwriting each other.
- A song that can't be exported - its file won't open, not every line is
  timed, or an output needs its audio file and it's not where the project
  says - is skipped with the reason, and the rest carry on. The status
  bar lists every file written and every song skipped.

## Pre-export checklist

The export dialog runs a quick checklist over whatever you're about to
export and lists anything worth a look - **warnings** (⚠, will visibly
look wrong) first, then **notes** (ℹ, often fine). None of them stop the
export; only missing timing does that. Each item has a **Show** button
that opens the line in the fine-tune panel, scrolls the timing table to
it and moves playback to just before it.

- **Line too long for `.cdg`** (`.cdg` only) - a row fits 48 characters;
  the rest is cut off. **Split line** splits it into two lines at the
  middle word, keeping its timing (the split falls where the second
  half's first word is sung) and updating the lyrics box to match.
  Ctrl+Z undoes it.
- **Characters the `.cdg` can't draw** (`.cdg` only) - see "Known
  limitations" below.
- **Lines that start after the song ends** - never shown.
- **Lines sung for under 0.3s** - usually a double-tap; the whole wipe
  flashes by.
- **Words highlighted for under 60ms** (note) - flash rather than wipe.
- **Lines still on estimated word timing** (note) - fine for a quick
  export; click words or run Auto-align for an accurate wipe.

## Known limitations / ideas for extending it

- In the `.cdg`, text wider than 48 characters gets clipped - the
  export dialog's checklist flags these lines and can split them for you.
- The `.cdg` font is a fixed bitmap font covering ASCII and Latin-1
  (é, ñ, ü, ç, ß, ¿, ...). Curly quotes, long dashes and ellipses (as
  pasted from most lyrics sites) draw as their plain equivalents, and
  other accented Latin letters (ő, š, ł, ...) as their base letter -
  your lyric text itself is never changed. Anything else (non-Latin
  scripts, emoji) still renders blank in the `.cdg`; the export dialog
  lists any such characters before you export. The video export uses a
  real font and isn't affected - and if your chosen lyric font is missing
  a curly quote or accented letter, the same plain stand-in is drawn
  instead of leaving a gap.
- Tapping sets both the start and end of each line (two taps), but the
  word-by-word wipe *within* that window is still an *estimate* by default
  (word count x ~0.45s/word) unless you fine-tune specific words via the
  "Words" panel or the timeline's word bubbles, or run "🪄 Auto-align
  words" (see "Auto-aligning word timing" above) to fill it in from real
  audio instead of guessing. The estimate's constants live at the top of
  `src/lyrics.rs` (`SECONDS_PER_WORD`, `MIN_SING_DURATION`) if you want to
  tune the default instead.
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
  less to scroll through. While the song plays, the table also scrolls
  itself to keep the line being timed in view (the next line to tap, or
  the line the fine-tune panel is following) - turn off **Follow current
  line** above the table to scroll it yourself. It only moves when the
  next line comes up, so scrolling by hand in between isn't fought.
  Drag the divider between the side panels and
  the middle to resize if you want more room - it's remembered across
  restarts.
- The live preview's default **Video** mode is a second renderer reading
  the same timing data (switch to **.cdg (exact)** to see the real `.cdg`
  output decoded) - see "Live preview vs. the exported file" above. Its
  Video mode doesn't draw the video export's text outline/drop shadow;
  those only appear in the exported `.mp4`.
- Slowed-down playback (0.5x / 0.75x) lowers the pitch along with the
  tempo - it's a timing aid, not a pitch-preserving time-stretch, and it
  never affects exports.
- Vocal removal for exports (instrumental/vocals audio, the video's
  "Remove vocals", the MP3+G zip's instrumental) always uses one model
  (`UVR-MDX-NET-Inst_HQ_3.onnx`) - only auto-align's own vocal isolation
  has a model choice, and there's no way to tune the model's parameters
  (segment size, overlap, etc.) yet.
- Exported `.mp4` video defaults to **AV1** (via the bundled SVT-AV1
  encoder, BSD-2-Clause-Patent - see `src/video.rs`), a real
  quality-targeted (CRF) encoder: one `crf` value (32, picked for this
  app's own mostly-static lyric-text-over-a-background content) gives
  consistent perceptual quality regardless of resolution, with no
  per-resolution bitrate guessing needed, and meaningfully smaller files
  than H.264 at equal quality. **H.264** (via the bundled `openh264`
  encoder - see `ffmpeg_path.rs` for why openh264 rather than the
  GPL-licensed x264 most ffmpeg builds default to) is kept selectable in
  the export dialog for compatibility - older devices/software, or a
  downstream tool that expects H.264 specifically. Unlike AV1, openh264
  has no CRF-style "target a quality level, let file size float" mode, so
  the H.264 option uses a fixed bitrate (`src/video.rs` - 3M for 720p,
  5M for 1080p, 14M for 4K, in either orientation - already tuned down once against real exports to avoid
  needlessly huge files for this app's low-motion content) rather than
  genuinely optimized encoding. The export dialog discloses this
  trade-off next to the codec picker rather than silently encoding
  whichever one is default.
