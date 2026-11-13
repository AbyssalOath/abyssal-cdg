// Windows GUI apps default to the "console" subsystem, which pops up (and
// keeps open) a terminal window behind the app for the lifetime of the
// process. Switching to the "windows" subsystem in release builds hides
// that window - debug builds keep the console so `println!`/panic output
// is still visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod align;
mod audio;
mod cdg;
mod cdg_decode;
mod checks;
mod codex;
mod ctc;
mod export;
mod ffmpeg_path;
mod font;
mod fonts;
mod formats;
mod lyrics;
mod mdx;
mod model_assets;
mod onboarding;
mod onnxrt;
mod prefs;
mod project;
mod recent;
mod stft;
mod timeline;
mod video;
mod vocals;
mod waveform;
mod zipfile;

use audio::AudioPlayer;
use eframe::egui;
use export::Palette;
use lyrics::{
    countdown_window, group_into_blocks, hide_upcoming_lines, singer_legend, LyricLine, Singer,
    TimedLine,
};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use video::{Resolution, VideoCodec, VideoPalette};

fn format_time(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "--:--".to_string();
    }
    let total_ds = (secs * 10.0).round() as i64; // deciseconds
    let m = total_ds / 600;
    let s = (total_ds / 10) % 60;
    let d = total_ds % 10;
    format!("{:02}:{:02}.{}", m, s, d)
}

/// Naming convention this app looks for on import to auto-fill the title/
/// artist fields, so users importing a batch of songs don't have to type
/// them in by hand: an audio file named "Artist - Song Name.ext". Returns
/// `None` (leaving the fields alone) when the file's name (extension
/// stripped) doesn't contain a " - " separator, or either side of it is
/// empty after trimming - safer than guessing wrong from an unrelated
/// filename.
fn parse_artist_title_from_filename(path: &Path) -> Option<(String, String)> {
    let stem = path.file_stem()?.to_str()?;
    let (artist, title) = stem.split_once(" - ")?;
    let artist = artist.trim();
    let title = title.trim();
    if artist.is_empty() || title.is_empty() {
        return None;
    }
    Some((artist.to_string(), title.to_string()))
}

/// Applies one checkbox click in the timing table's selection column to
/// `selected`/`anchor` - plain checkbox semantics: a click toggles just
/// this line's membership without touching the rest, so several lines can
/// be ticked one by one; Shift-click selects every line between the anchor
/// and this one (inclusive, in either direction), replacing the selection
/// with that range rather than adding to it. Every click moves the anchor
/// to `i` afterward, so a Shift-click chain extends from wherever you last
/// clicked, not always from the very first click. A free function (rather than a `KaraokeApp` method) purely so it
/// can be unit-tested directly, the same way `parse_artist_title_from_filename`
/// above is.
fn apply_row_selection_click(
    selected: &mut std::collections::BTreeSet<usize>,
    anchor: &mut Option<usize>,
    i: usize,
    shift: bool,
) {
    if shift {
        let a = anchor.unwrap_or(i);
        let (lo, hi) = (a.min(i), a.max(i));
        *selected = (lo..=hi).collect();
    } else if !selected.remove(&i) {
        selected.insert(i);
    }
    *anchor = Some(i);
}

/// Folds one edit of the lyrics text box into `edited` - the span of the
/// box's text (in char offsets) changed since the last parse. `cursor` is
/// the cursor's char offset right after the edit and `delta` how many chars
/// it added (negative if it removed some): a typed or pasted run ends at the
/// cursor, so it spans `cursor - delta..cursor`. Offsets in the earlier span
/// past the edit point are shifted along with the text. Best-effort (e.g. a
/// paste over a selection is treated as a plain insert) - it's only a
/// tie-breaker hint for `merge_reparsed_lyrics`. Free function so it can be
/// unit-tested directly.
fn track_lyrics_edit(
    edited: Option<std::ops::Range<usize>>,
    cursor: usize,
    delta: isize,
) -> std::ops::Range<usize> {
    let at = cursor.saturating_sub(delta.max(0) as usize);
    let shift = |x: usize| {
        if x <= at {
            x
        } else {
            (x as isize + delta).max(at as isize) as usize
        }
    };
    match edited {
        Some(r) => shift(r.start).min(at)..shift(r.end).max(cursor),
        None => at..cursor,
    }
}

/// Converts a char-offset span of `text` (see [`track_lyrics_edit`]) into
/// the range of `text.lines()` indices it touches - always at least the one
/// line the span starts on, so a pure deletion still marks its line.
fn char_span_to_line_range(text: &str, span: std::ops::Range<usize>) -> std::ops::Range<usize> {
    let line_of = |offset: usize| text.chars().take(offset).filter(|&c| c == '\n').count();
    let first = line_of(span.start);
    let last = if span.end > span.start {
        line_of(span.end - 1)
    } else {
        first
    };
    first..last + 1
}

/// Convert a CDG 4-bit-per-channel color to a full-range egui color for UI
/// display / color pickers.
fn color32_from_cdg(c: cdg::CdgColor) -> egui::Color32 {
    egui::Color32::from_rgb(c.r * 17, c.g * 17, c.b * 17)
}

/// Convert an egui color back down to CDG's 4-bit-per-channel color.
fn cdg_from_color32(c: egui::Color32) -> cdg::CdgColor {
    cdg::CdgColor::new(
        (c.r() as u16 * 15 / 255) as u8,
        (c.g() as u16 * 15 / 255) as u8,
        (c.b() as u16 * 15 / 255) as u8,
    )
}

/// Convert a full-range egui color to the plain RGB struct project files are
/// saved with (kept independent of `egui` so `project.rs` doesn't need
/// `egui`'s `serde` feature enabled just for this one struct).
fn rgb_color_from_color32(c: egui::Color32) -> project::RgbColor {
    project::RgbColor {
        r: c.r(),
        g: c.g(),
        b: c.b(),
    }
}

fn color32_from_rgb_color(c: project::RgbColor) -> egui::Color32 {
    egui::Color32::from_rgb(c.r, c.g, c.b)
}

/// A one-line `LayoutJob` for `text` with the karaoke color wipe applied
/// at `fraction` (0.0-1.0) of its characters - `highlight` up to there,
/// `unsung` after. Same per-character rounding the live preview's main
/// line uses.
fn wipe_layout_job(
    text: &str,
    fraction: f32,
    font_id: egui::FontId,
    unsung: egui::Color32,
    highlight: egui::Color32,
) -> egui::text::LayoutJob {
    let chars: Vec<char> = text.chars().collect();
    let split = ((fraction * chars.len().max(1) as f32).round() as usize).min(chars.len());
    let sung: String = chars[..split].iter().collect();
    let rest: String = chars[split..].iter().collect();
    let mut job = egui::text::LayoutJob::default();
    for (part, color) in [(sung, highlight), (rest, unsung)] {
        if !part.is_empty() {
            job.append(
                &part,
                0.0,
                egui::TextFormat {
                    color,
                    font_id: font_id.clone(),
                    ..Default::default()
                },
            );
        }
    }
    job
}

/// Fades `color` toward transparent (letting whatever's drawn underneath -
/// the live preview's own mock-screen background - show through), rather
/// than toward black the way `Color32::gamma_multiply` would. Used by
/// `draw_preview` for the same fade-in/fade-out treatment `render_frame`
/// (video.rs) applies via `Canvas::blend_pixel`'s own opacity multiplier -
/// see `lyrics::sung_line_fade_alpha`/`reveal_fade_alpha`.
fn faded_color(color: egui::Color32, alpha: f32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (color.a() as f32 * alpha.clamp(0.0, 1.0)).round() as u8,
    )
}

/// The export file name a song gets by default: its audio file's name
/// (extension stripped), else its title, else "karaoke" - sanitized (see
/// [`sanitize_export_base_name`]).
fn default_export_base_name(audio_path: Option<&Path>, title: &str) -> String {
    audio_path
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            let t = title.trim();
            (!t.is_empty()).then(|| t.to_string())
        })
        .map(|s| sanitize_export_base_name(&s))
        .unwrap_or_else(|| "karaoke".to_string())
}

/// `base`, or `base (2)`, `base (3)`, ... - whichever isn't in `used` yet
/// (case-insensitively, since Windows/macOS file names are) - and records
/// it as used. Keeps two songs in one batch export that would share a file
/// name from overwriting each other.
fn unique_base_name(base: &str, used: &mut std::collections::HashSet<String>) -> String {
    let mut candidate = base.to_string();
    let mut n = 2;
    while !used.insert(candidate.to_lowercase()) {
        candidate = format!("{base} ({n})");
        n += 1;
    }
    candidate
}

/// Batch export: every project in `projects`, one after another, with the
/// same export choices (`opts`) into `folder` - each song with its own
/// colors, video settings, font, background and audio from its project
/// file. A song that can't be exported (unreadable file, untimed lines,
/// missing audio an output needs) is skipped with the reason, and the rest
/// carry on. Overall progress (0.0-1.0) goes to `progress`.
fn run_batch_export(
    projects: &[PathBuf],
    opts: &ExportOptions,
    folder: &Path,
    progress: &dyn Fn(f32),
) -> Vec<ExportOutcome> {
    let mut outcomes = Vec::new();
    let mut used_names = std::collections::HashSet::new();
    let n = projects.len().max(1) as f32;
    for (i, path) in projects.iter().enumerate() {
        progress(i as f32 / n);
        let file_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("project")
            .to_string();
        let skip = |outcomes: &mut Vec<ExportOutcome>, reason: String| {
            outcomes.push(ExportOutcome {
                label: file_name.clone().into(),
                result: Err(format!("skipped - {reason}")),
            });
        };
        let project = match project::ProjectFile::load_from_file(path) {
            Ok(p) => p,
            Err(e) => {
                skip(&mut outcomes, format!("couldn't open it: {e}"));
                continue;
            }
        };
        let inputs = ExportInputs::from_project(project);
        if opts.any_timed() {
            if let Some(reason) = missing_timing(&inputs.lines) {
                skip(&mut outcomes, reason);
                continue;
            }
        }
        if opts.needs_audio() && inputs.audio_path.is_none() {
            skip(
                &mut outcomes,
                "its audio file wasn't found (video/instrumental/vocals/zip need it)".to_string(),
            );
            continue;
        }
        let base = unique_base_name(
            &default_export_base_name(inputs.audio_path.as_deref(), &inputs.title),
            &mut used_names,
        );
        let song = run_export(&inputs, opts, folder, &base, &|p| {
            progress((i as f32 + p.clamp(0.0, 1.0)) / n)
        });
        for o in song {
            outcomes.push(ExportOutcome {
                label: format!("{file_name} - {}", o.label).into(),
                result: o.result,
            });
        }
    }
    progress(1.0);
    outcomes
}

/// Reduces `base` to a plain filename with no path components, for safe
/// use as the shared base name every export output path is built from
/// (`{base}.cdg`, `{base}.mp4`, ...) - security-relevant, not just
/// tidiness: `export_base_name` can be auto-filled from the project's own
/// `title` field (see `open_export_dialog`), which comes straight from a
/// `.abyzl` project file with no validation, and project files are
/// explicitly meant to be shared between users. Without this, a crafted
/// title like "/etc/cron.d/evil" or "../../../elsewhere" would let a
/// shared project file silently redirect every export output to an
/// arbitrary location instead of the folder actually picked in the
/// export dialog. `Path::file_name()` already strips any directory
/// components/traversal *and* discards an absolute-path override
/// entirely (it only ever returns the final component, never lets one
/// path override another's root) - falls back to `"karaoke"` (this app's
/// own existing default) if nothing usable is left after that.
fn sanitize_export_base_name(base: &str) -> String {
    Path::new(base.trim())
        .file_name()
        .and_then(|f| f.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("karaoke")
        .to_string()
}

/// The song time a live tap at playback position `pos` should record:
/// `pos` minus the user's tap latency compensation (see `prefs.rs`), so a
/// tap that lands a consistent ~150ms after what was actually heard is
/// recorded where the sound really was. The latency is a wall-clock delay
/// (reaction time, audio output delay), so at a reduced playback `speed`
/// it covers proportionally less song time - 150ms at half speed is 75ms
/// of song. Never negative.
fn compensate_tap(pos: f64, latency_ms: f64, speed: f32) -> f64 {
    (pos - latency_ms / 1000.0 * speed as f64).max(0.0)
}

/// Which renderer the live preview panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum PreviewMode {
    /// The video-style preview (multi-line verse blocks, smooth wipe).
    #[default]
    Video,
    /// The real `.cdg` output, decoded (see `cdg_decode.rs`).
    Cdg,
}

/// State behind the preview's ".cdg" mode: the decoded stream for the
/// project as it was at `key` (a hash of everything the `.cdg` depends
/// on), its on-screen texture, and any background re-render in flight.
#[derive(Default)]
struct CdgPreviewState {
    current: Option<(u64, cdg_decode::CdgPlayback)>,
    texture: Option<egui::TextureHandle>,
    /// Packet position last uploaded to `texture`, to skip re-uploading an
    /// unchanged frame.
    shown_pos: Option<usize>,
    job: Option<(u64, std::sync::mpsc::Receiver<cdg_decode::CdgPlayback>)>,
    /// Last time the inputs were hashed - throttled, since a timeline drag
    /// changes them every frame.
    last_check: Option<std::time::Instant>,
    wanted_key: u64,
}

/// How often the `.cdg` preview re-checks whether the project changed.
const CDG_PREVIEW_RECHECK: std::time::Duration = std::time::Duration::from_millis(250);

/// The steps of making a karaoke song, as shown in the step bar across the
/// top of the window (see [`KaraokeApp::draw_step_bar`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkflowStep {
    Audio,
    Lyrics,
    Tap,
    FineTune,
    Export,
}

impl WorkflowStep {
    const ALL: [WorkflowStep; 5] = [
        WorkflowStep::Audio,
        WorkflowStep::Lyrics,
        WorkflowStep::Tap,
        WorkflowStep::FineTune,
        WorkflowStep::Export,
    ];

    fn label(self) -> &'static str {
        match self {
            WorkflowStep::Audio => "Audio",
            WorkflowStep::Lyrics => "Lyrics",
            WorkflowStep::Tap => "Tap",
            WorkflowStep::FineTune => "Fine-tune",
            WorkflowStep::Export => "Export",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            WorkflowStep::Audio => "Load the song's audio file.",
            WorkflowStep::Lyrics => "Paste or load the lyrics, then Parse lyrics.",
            WorkflowStep::Tap => "Play the song and tap Space at each line's start and end.",
            WorkflowStep::FineTune => {
                "Optional: time individual words (click them as they're sung, or Auto-align)."
            }
            WorkflowStep::Export => "Export your .cdg, video, lyrics or subtitles.",
        }
    }
}

/// How far along a step is, for the step bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StepStatus {
    Done,
    /// The step the "Next" action points at.
    Current,
    /// Not done, but not required either (fine-tuning words).
    Optional,
    Todo,
}

/// What the step bar needs to know about the project - a plain snapshot so
/// [`workflow_status`] can be tested without the app.
#[derive(Clone, Copy, Debug, Default)]
struct WorkflowSnapshot {
    has_audio: bool,
    /// Lyrics typed/pasted into the box (parsed or not).
    has_lyrics_text: bool,
    lines: usize,
    /// Lines with a start time.
    timed: usize,
    /// Timed lines whose word timing is still the estimate.
    estimated: usize,
    exported: bool,
}

/// Each step's status, and which step "Next" points at. Audio, Lyrics and
/// Tap are required, in order; Fine-tune is optional (never blocks
/// Export - it shows as Optional until every line's words are timed);
/// Export is next once everything's timed, and Done after an export.
fn workflow_status(s: &WorkflowSnapshot) -> ([StepStatus; 5], WorkflowStep) {
    let audio = s.has_audio;
    let lyrics = s.lines > 0;
    let tapped = lyrics && s.timed == s.lines;
    let fine_tuned = tapped && s.estimated == 0;
    let next = if !audio {
        WorkflowStep::Audio
    } else if !lyrics {
        WorkflowStep::Lyrics
    } else if !tapped {
        WorkflowStep::Tap
    } else {
        WorkflowStep::Export
    };
    let status = |step: WorkflowStep, done: bool| {
        if done {
            StepStatus::Done
        } else if step == next {
            StepStatus::Current
        } else if step == WorkflowStep::FineTune {
            StepStatus::Optional
        } else {
            StepStatus::Todo
        }
    };
    (
        [
            status(WorkflowStep::Audio, audio),
            status(WorkflowStep::Lyrics, lyrics),
            status(WorkflowStep::Tap, tapped),
            status(WorkflowStep::FineTune, fine_tuned),
            status(WorkflowStep::Export, s.exported),
        ],
        next,
    )
}

/// How a status message should look - see [`classify_status`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusKind {
    /// Routine progress (each tap, loop on/off...) - status line only.
    Info,
    Success,
    /// Something to do first, or a partial success.
    Warning,
    Error,
}

impl StatusKind {
    fn icon(self) -> &'static str {
        match self {
            StatusKind::Info => "",
            StatusKind::Success => "✔ ",
            StatusKind::Warning => "⚠ ",
            StatusKind::Error => "✖ ",
        }
    }

    fn color(self, visuals: &egui::Visuals) -> egui::Color32 {
        match self {
            StatusKind::Info => visuals.text_color(),
            StatusKind::Success => egui::Color32::from_rgb(90, 190, 110),
            StatusKind::Warning => egui::Color32::from_rgb(230, 160, 40),
            StatusKind::Error => egui::Color32::from_rgb(235, 85, 85),
        }
    }

    /// How long a toast for this kind stays up (none for Info).
    fn toast_secs(self) -> Option<f32> {
        match self {
            StatusKind::Info => None,
            StatusKind::Success => Some(4.0),
            StatusKind::Warning => Some(6.0),
            StatusKind::Error => Some(8.0),
        }
    }
}

/// Classifies a status message by its wording (judged on its first line),
/// so the ~100 places that set `status` don't each need to say how it
/// should look. Warnings are checked first: "finished with some errors" is
/// a partial success, and "... first" messages are requests, not failures.
fn classify_status(status: &str) -> StatusKind {
    let first = status.lines().next().unwrap_or("").to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| first.contains(w));
    if has(&[
        "some errors",
        "wasn't found",
        "skipped",
        " first",
        "nothing to",
        "already",
        "no line to",
        "found no",
    ]) {
        StatusKind::Warning
    } else if has(&[
        "failed",
        "error",
        "couldn't",
        "can't",
        "cannot",
        "must ",
        "isn't a supported",
        "no longer exists",
    ]) {
        StatusKind::Error
    } else if has(&[
        "export complete",
        "saved project",
        "audio loaded",
        "loaded project",
        "recovered",
        "auto-aligned",
        "split line",
        "shifted",
    ]) {
        StatusKind::Success
    } else {
        StatusKind::Info
    }
}

/// A color as the `.cdg` will actually show it: reduced to CDG's 16 levels
/// per channel (see `cdg_from_color32`), then back to 8-bit.
fn as_cdg_rgb(c: egui::Color32) -> [f32; 3] {
    let q = cdg_from_color32(c);
    [q.r as f32 * 17.0, q.g as f32 * 17.0, q.b as f32 * 17.0]
}

/// WCAG relative luminance of an 8-bit RGB color.
fn relative_luminance(rgb: [f32; 3]) -> f32 {
    let lin = |v: f32| {
        let c = v / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
}

/// WCAG contrast ratio between two colors (1.0 = identical, 21.0 = black
/// on white). 3:1 is the usual minimum for large text like lyrics.
fn contrast_ratio(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Minimum contrast for lyric/title text against the background.
const MIN_TEXT_CONTRAST: f32 = 3.0;
/// The dim next-line preview is meant to be faint, but still legible.
const MIN_PREVIEW_CONTRAST: f32 = 1.8;
/// Upcoming vs sung closer than this (RGB distance, as the `.cdg` shows
/// them) and the wipe is hard to see.
const MIN_WIPE_DISTANCE: f32 = 60.0;

/// Readability problems with `colors` for the voices in `voices` - judged
/// on the colors as the `.cdg` will show them (16 levels per channel, so
/// two picked colors can end up identical), which also covers the video.
/// Checks each voice's upcoming vs sung colors (is the wipe visible?), all
/// text against the background (WCAG contrast), and voices whose sung
/// colors look the same (can singers tell whose line it is?).
fn color_warnings(colors: &ExportColors, voices: &[Singer]) -> Vec<String> {
    let bg = as_cdg_rgb(colors.bg);
    let mut out = Vec::new();
    let dist = |a: [f32; 3], b: [f32; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };
    let check_text = |out: &mut Vec<String>, what: &str, c: egui::Color32, min: f32| {
        let ratio = contrast_ratio(as_cdg_rgb(c), bg);
        if ratio < min {
            out.push(format!(
                "{what} is hard to read on the background (contrast {ratio:.1}:1 - aim for \
                 {min:.0}:1 or more). Try a lighter color, or a darker background."
            ));
        }
    };
    for &v in voices {
        let (unsung, sung) = colors.singer_pair(v);
        let (u, h) = (as_cdg_rgb(unsung), as_cdg_rgb(sung));
        if u == h {
            out.push(format!(
                "{}: the upcoming and sung colors come out identical in the .cdg (it only has \
                 16 shades per color channel) - the wipe won't show.",
                v.label()
            ));
        } else if dist(u, h) < MIN_WIPE_DISTANCE {
            out.push(format!(
                "{}: the upcoming and sung colors are very close - the wipe will be hard to see.",
                v.label()
            ));
        }
        check_text(
            &mut out,
            &format!("{} upcoming text", v.label()),
            unsung,
            MIN_TEXT_CONTRAST,
        );
        check_text(
            &mut out,
            &format!("{} sung text", v.label()),
            sung,
            MIN_TEXT_CONTRAST,
        );
    }
    check_text(&mut out, "The title", colors.title, MIN_TEXT_CONTRAST);
    check_text(&mut out, "The artist", colors.artist, MIN_TEXT_CONTRAST);
    check_text(
        &mut out,
        "The next-line preview",
        colors.preview,
        MIN_PREVIEW_CONTRAST,
    );
    // Default is the "not assigned yet" voice and deliberately starts out
    // looking like Male, so it's left out of the "same color" comparison.
    let named: Vec<Singer> = voices
        .iter()
        .copied()
        .filter(|v| *v != Singer::Default)
        .collect();
    for (k, &a) in named.iter().enumerate() {
        for &b in &named[k + 1..] {
            if dist(
                as_cdg_rgb(colors.singer_pair(a).1),
                as_cdg_rgb(colors.singer_pair(b).1),
            ) < MIN_WIPE_DISTANCE
            {
                out.push(format!(
                    "{} and {} sing in nearly the same color - singers may not tell whose line \
                     is whose.",
                    a.label(),
                    b.label()
                ));
            }
        }
    }
    out
}

/// The "Copy timing" dialog's state (see [`KaraokeApp::draw_copy_timing_dialog`]).
struct CopyTimingDialog {
    /// The lines whose timing is copied (sorted).
    sources: Vec<usize>,
    /// First line of the repeat being timed.
    first_target: usize,
}

/// A timing-table row's status dot (see [`line_status`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineStatus {
    /// No start time yet.
    Untimed,
    /// Timed, but its word timing is still the even estimate.
    Estimated,
    /// Timed, and its words are set (by hand, auto-align or an import) -
    /// or it's a one-word line with nothing more to fine-tune.
    FineTuned,
}

impl LineStatus {
    fn color(self, visuals: &egui::Visuals) -> egui::Color32 {
        match self {
            LineStatus::Untimed => egui::Color32::from_rgb(230, 160, 40),
            LineStatus::Estimated => visuals.weak_text_color(),
            LineStatus::FineTuned => STEP_DONE,
        }
    }

    fn describe(self) -> &'static str {
        match self {
            LineStatus::Untimed => "Not timed yet - tap it, or type its start.",
            LineStatus::Estimated => {
                "Timed - word timing is still estimated (click its words as they're sung, \
                 or Auto-align)."
            }
            LineStatus::FineTuned => "Timed and fine-tuned.",
        }
    }
}

/// Each line's status, in order (see [`LineStatus`]).
fn line_statuses(lines: &[LyricLine]) -> Vec<LineStatus> {
    let estimated: std::collections::HashSet<usize> = checks::lines_on_estimated_timing(lines)
        .into_iter()
        .collect();
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if l.start.is_none() {
                LineStatus::Untimed
            } else if estimated.contains(&i) {
                LineStatus::Estimated
            } else {
                LineStatus::FineTuned
            }
        })
        .collect()
}

/// Every keyboard shortcut, grouped, for the F1 / ? cheat sheet (see
/// [`KaraokeApp::draw_shortcuts_window`]). Keep in step with
/// `handle_keyboard_shortcuts`. "Current line" = the line open in the
/// fine-tune panel, else the line most recently tapped.
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "Tapping",
        &[
            (
                "Space",
                "Tap the next line's start / end (pause/play once all timed)",
            ),
            ("Shift+Space", "End this line and start the next in one tap"),
            ("Enter", "Re-tap the current line's start"),
            (
                "M / F / D / S / B",
                "Voice: Male / Female / Duet / Screaming / Backing",
            ),
        ],
    ),
    (
        "Fixing timing",
        &[
            (
                ", / .",
                "Nudge the current line's start 0.1s earlier / later",
            ),
            (
                "Shift+, / Shift+.",
                "Shift the selected lines 0.1s earlier / later",
            ),
            ("Delete", "Clear the current line's timing"),
            ("Ctrl+Z / Ctrl+Shift+Z", "Undo / redo (Ctrl+Y redoes too)"),
        ],
    ),
    (
        "Playback",
        &[
            ("← / →", "Skip back / forward 2 seconds"),
            ("Home", "Back to the start"),
            ("L", "Loop the current line on / off"),
        ],
    ),
    (
        "Files",
        &[
            ("Ctrl+S", "Save project"),
            ("Ctrl+Shift+S", "Save project as…"),
            ("Ctrl+E", "Export…"),
            ("Ctrl+= / Ctrl+-", "Bigger / smaller UI (saved)"),
            ("Ctrl+0", "UI back to 100%"),
            ("F1 or ?", "Show / hide this list"),
        ],
    ),
];

/// A time typed into the transport's time readout: `MM:SS(.ss)` (as the
/// timing table accepts), `H:MM:SS`, or plain seconds. `None` if it's not
/// a time, or negative.
fn parse_jump_time(text: &str) -> Option<f64> {
    let text = text.trim();
    let parts: Vec<&str> = text.split(':').collect();
    let secs = match parts.as_slice() {
        [s] => s.parse::<f64>().ok()?,
        [m, s] => m.parse::<f64>().ok()? * 60.0 + s.parse::<f64>().ok()?,
        [h, m, s] => {
            h.parse::<f64>().ok()? * 3600.0
                + m.parse::<f64>().ok()? * 60.0
                + s.parse::<f64>().ok()?
        }
        _ => return None,
    };
    (secs.is_finite() && secs >= 0.0).then_some(secs)
}

/// Opens `dir` in the system file manager (Explorer, Finder, or whatever
/// `xdg-open` picks). Fire-and-forget: the file manager's own exit status
/// isn't meaningful (Explorer returns 1 on success).
fn open_in_file_manager(dir: &Path) -> std::io::Result<()> {
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(dir)
        .spawn()
        .map(|_| ())
}

/// An explanation shown under a control. With tips on (see the "Tips"
/// toggle), the full `detail` as small grey text, as always; with tips off,
/// just the one-line `summary` plus an ⓘ that shows `detail` on hover -
/// nothing is lost, it's just out of the way. An empty `summary` hides the
/// text entirely when tips are off (used for pure tips).
fn help_text(ui: &mut egui::Ui, show_tips: bool, summary: &str, detail: impl Into<String>) {
    let detail: String = detail.into();
    if show_tips {
        ui.label(egui::RichText::new(detail).small().weak());
    } else if !summary.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(summary).small().weak());
            ui.label(egui::RichText::new("ⓘ").small().weak())
                .on_hover_text(detail);
        });
    }
}

/// The app's accent color (the "Abyssal" purple) - primary actions and the
/// current step.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(140, 82, 220);
const STEP_DONE: egui::Color32 = egui::Color32::from_rgb(70, 160, 95);

/// An active playback loop (see [`KaraokeApp::apply_loop`]).
#[derive(Clone, Copy, Debug, PartialEq)]
enum LoopMode {
    /// Repeat one lyric line (index into `lines`), with a little lead-in
    /// and tail - its bounds are re-read every frame, so retiming the
    /// line's words or dragging its bubble moves the loop along with it.
    Line(usize),
    /// A free loop between two hand-set points. `b` is `None` between
    /// pressing A and pressing B; nothing loops until both are set.
    Ab { a: f64, b: Option<f64> },
}

/// Whether playback moving from `prev` (last frame's position) to `pos`
/// crossed the loop end `to` - only a forward crossing from inside counts,
/// so seeking past the loop by hand isn't yanked back, and the very first
/// frame of playback (`prev` = `None`) never wraps.
fn loop_should_wrap(prev: Option<f64>, pos: f64, to: f64) -> bool {
    prev.is_some_and(|p| p < to) && pos >= to
}

/// How much audio a line loop plays before the line starts / after it
/// ends - enough to hear the lead-in and catch the first/last word.
const LOOP_PRE_ROLL_SECS: f64 = 1.0;
const LOOP_POST_ROLL_SECS: f64 = 0.5;
/// Shortest allowed A-B loop.
const MIN_AB_LOOP_SECS: f64 = 0.25;

/// Which timestamp clicking a word in the fine-tune panel sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum WordTapMode {
    #[default]
    Start,
    End,
}

/// Which timestamp the "Tap next line" button/spacebar registers next for
/// `self.next_untimed` - alternates per line: start, then end, then the
/// next line's start, and so on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum TapPhase {
    #[default]
    Start,
    End,
}

/// Cursor to show while hovering (or actively dragging) a timeline bubble -
/// `active_mode` is `Some` while a drag on *this* bubble is already under
/// way (so the icon doesn't flicker to whatever's under the pointer as it
/// moves outside the bubble's own edges mid-drag); otherwise it's derived
/// from where within the bubble the pointer currently is.
fn timeline_cursor_icon(
    active_mode: Option<timeline::DragMode>,
    response: &egui::Response,
    bubble_rect: egui::Rect,
) -> Option<egui::CursorIcon> {
    let mode = match active_mode {
        Some(m) => m,
        None => {
            let pos = response.hover_pos()?;
            timeline::classify_drag(pos.x - bubble_rect.left(), bubble_rect.width())
        }
    };
    Some(match mode {
        timeline::DragMode::Body => {
            if response.dragged() {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            }
        }
        timeline::DragMode::LeftEdge | timeline::DragMode::RightEdge => {
            egui::CursorIcon::ResizeHorizontal
        }
    })
}

/// A "start a new project" or "load this project" request, held while
/// waiting on the user to resolve unsaved changes first - see
/// `KaraokeApp::pending_unsaved_action`.
enum PendingUnsavedAction {
    NewProject,
    LoadProject(PathBuf),
}

struct KaraokeApp {
    audio: Option<AudioPlayer>,
    audio_error: Option<String>,

    lyrics_raw: String,
    /// Char-offset span of `lyrics_raw` typed/pasted into since the last
    /// parse (see [`track_lyrics_edit`]) - tells "Parse lyrics" which copy of
    /// a repeated line is the new one. `None` when there's no such edit to
    /// go on (fresh parse, undo, load), which just means no tie-break hint.
    lyrics_edited_span: Option<std::ops::Range<usize>>,
    lines: Vec<LyricLine>,
    /// Path this project was last saved to or loaded from - Ctrl+S / "Save
    /// Project" writes straight back here without a dialog; `None` means
    /// there's no file yet, so saving falls back to "Save Project As…".
    current_project_path: Option<PathBuf>,
    /// A crash-recovery autosave found at startup, waiting on a yes/no
    /// prompt before anything else is drawn - see `draw_recovery_prompt`.
    pending_recovery: Option<project::ProjectFile>,
    /// Last time the crash-recovery autosave was written, so it only
    /// happens every [`AUTOSAVE_INTERVAL`] rather than every frame.
    last_autosave: Option<std::time::Instant>,
    /// The project state as of the last successful explicit save this
    /// session (`None` if never saved). Compared against the live state on
    /// exit to decide whether the crash-recovery autosave should be kept
    /// around for next launch - if they differ, there's unsaved work, so
    /// the autosave stays *regardless of whether this was a clean exit or
    /// a crash* - an accidental window close with unsaved changes deserves
    /// the same recovery prompt a crash would get, not just intentional
    /// crashes. Deliberately *not* updated by "Recover" on the
    /// crash-recovery prompt - recovered content is exactly the kind of
    /// unsaved work this exists to protect, so it stays flagged as such
    /// until explicitly saved.
    last_saved_snapshot: Option<project::ProjectFile>,
    /// Set while the "quit without saving?" confirmation is showing (in
    /// place of the rest of the UI) - triggered by intercepting a window
    /// close request when there are unsaved changes.
    show_quit_confirm: bool,
    /// Set once the user has explicitly chosen to quit (with or without
    /// saving first) from that confirmation - the next close request is
    /// let through instead of being intercepted again.
    quit_confirmed: bool,
    /// A "start a new project" or "load this project" request made while
    /// there are unsaved changes to the currently open project - set
    /// instead of acting immediately, so `draw_unsaved_action_confirm_prompt`
    /// can offer to save first (same "don't silently discard work"
    /// protection window close already has via `show_quit_confirm`,
    /// applied to every way a load can be triggered - the file picker, the
    /// Recent menu, dragging a `.abyzl` file onto the window - and to
    /// "New Project"). `None` means no confirmation is currently pending.
    pending_unsaved_action: Option<PendingUnsavedAction>,
    /// Index of the next line "Tap next line"/Space will assign a
    /// timestamp to.
    next_untimed: usize,
    /// Whether that next tap sets `next_untimed`'s start or end - see
    /// [`TapPhase`].
    tap_phase: TapPhase,
    /// If set, the fine-tune-words panel is open for this line index.
    word_tap_line: Option<usize>,
    /// Whether clicking a word in the fine-tune panel sets its start or end
    /// time - see [`WordTapMode`].
    word_tap_mode: WordTapMode,
    /// Whether the fine-tune-words panel automatically switches to whichever
    /// line is currently playing (see [`Self::auto_follow_word_tap_line`]).
    /// On by default, but it can make a line's very first/last word hard to
    /// catch - the panel jumps to/away from the line right as it starts/
    /// ends, the same instant those words need a click - so it can be
    /// turned off to fine-tune a line at your own pace instead.
    auto_follow_words: bool,
    /// Whether the timing table's Lyric column truncates long lines (with a
    /// hover tooltip for the full text) instead of growing to fit - off by
    /// default (nothing is hidden until you ask for it), a session-only
    /// view preference like `auto_follow_words`, not saved with the
    /// project. Narrows the table considerably on a long lyric line, so
    /// there's less to horizontally scroll through to reach the Singer/
    /// Countdown/action columns.
    compact_timing_table: bool,
    /// Whether the timing table scrolls itself to keep the line currently
    /// being timed (the next tap-along line, or the line the fine-tune
    /// panel is following once tapping's done) in view while the song
    /// plays. Session-only, on by default.
    follow_timing_table: bool,
    /// The row the timing table last auto-scrolled to, so it only scrolls
    /// when the followed line *changes* - leaving the user free to scroll
    /// the table by hand in between without it snapping straight back.
    timing_table_scrolled_to: Option<usize>,
    /// One-shot request (e.g. the export checklist's "Show") to scroll the
    /// timing table to this row next frame, regardless of playback.
    timing_table_scroll_request: Option<usize>,
    /// Per-machine preferences (see `prefs.rs`) - currently the tap
    /// latency compensation applied by [`compensate_tap`].
    prefs: prefs::Prefs,
    /// How far the "Shift selection" buttons move the selected lines, in
    /// seconds. Session-only.
    shift_amount_secs: f64,
    /// The active playback loop, if any. Session-only.
    loop_mode: Option<LoopMode>,
    /// Playback position as of the previous frame, so [`Self::apply_loop`]
    /// only jumps back when playback *crosses* the loop's end - seeking
    /// past it by hand is left alone.
    loop_last_pos: Option<f64>,
    /// Whether the live preview is shown in its own separate, resizable
    /// window (see [`KaraokeApp::show_preview_window`]) instead of the
    /// right panel. Session-only like `compact_timing_table` - not saved,
    /// so the app never opens with an extra window you didn't just ask for.
    preview_popped_out: bool,
    /// Video-style or exact-`.cdg` live preview. Session-only.
    preview_mode: PreviewMode,
    cdg_preview: CdgPreviewState,
    /// Line index + in-progress text while a start-time field in the
    /// timing table is focused - `None` the rest of the time, so the
    /// displayed text otherwise always mirrors the line's live value.
    start_edit: Option<(usize, String)>,
    /// Same as `start_edit`, for the end-time field.
    end_edit: Option<(usize, String)>,
    /// Line indices currently selected in the timing table's checkbox
    /// column, for bulk actions (currently just "set voice for selection"),
    /// driven by [`apply_row_selection_click`]. Not persisted (project
    /// save/load, undo/redo) since it's pure transient UI state, and
    /// clamped to `self.lines`'s current length wherever it's read, since
    /// `self.lines` can shrink out from under it (re-parsing, undo, loading
    /// a different file, ...).
    selected_lines: std::collections::BTreeSet<usize>,
    /// The last line clicked in the timing table's checkbox column, i.e.
    /// where a subsequent Shift-click range-select extends from. `None`
    /// means nothing's been clicked yet this session (Shift-click with no
    /// anchor falls back to selecting just the clicked line).
    selection_anchor: Option<usize>,

    title: String,
    artist: String,

    color_bg: egui::Color32,
    /// Color for a line whose singer hasn't been manually set - defaults to
    /// the same look as `color_male_unsung`/`color_male_highlight`, but
    /// independently adjustable (e.g. to match a background image/video).
    color_default_unsung: egui::Color32,
    color_default_highlight: egui::Color32,
    color_male_unsung: egui::Color32,
    color_male_highlight: egui::Color32,
    color_female_unsung: egui::Color32,
    color_female_highlight: egui::Color32,
    color_duet_unsung: egui::Color32,
    color_duet_highlight: egui::Color32,
    color_preview: egui::Color32,
    color_title: egui::Color32,
    color_artist: egui::Color32,
    color_screaming_unsung: egui::Color32,
    color_screaming_highlight: egui::Color32,
    color_backing_unsung: egui::Color32,
    color_backing_highlight: egui::Color32,
    /// Which preset (if any) the current colors were last set from - purely
    /// cosmetic (which option the dropdown shows selected); manually
    /// tweaking an individual color afterward doesn't change or clear it.
    color_preset: ColorPreset,

    /// Recently opened/saved project and audio files, for the "Recent"
    /// menu - persisted to disk (see `recent.rs`) so it survives restarts.
    recent_files: recent::RecentFiles,

    status: String,

    /// While the seek bar is being dragged, holds the not-yet-committed
    /// position so the displayed handle doesn't fight with the live
    /// playback position updating every frame.
    seek_drag_value: Option<f64>,

    video_resolution: Resolution,
    /// Which codec the video export uses - see [`VideoCodec`] for why AV1
    /// is the default and H.264 is kept as a selectable, disclosed-as-
    /// larger fallback.
    video_codec: VideoCodec,
    /// Video orientation/lyric size/outline/shadow/fade (saved with the
    /// project, like `video_codec`).
    video_style: video::VideoStyle,
    /// The outro card settings (saved with the project).
    outro: lyrics::OutroSettings,
    /// Whether the video export/preview shows a small credit line under
    /// the title card (see `video.rs`'s `CREDIT_TEXT`) - on by default,
    /// but never forced: this is the user's own toggle, not a watermark
    /// they can't remove.
    show_credit_line: bool,
    /// An image/video shown behind the lyrics in the video export/preview
    /// instead of a flat `color_bg` fill - `None` means the plain
    /// solid-color look, unchanged from before this existed. Never applies
    /// to the `.cdg` export (see [`video::Background`]).
    background: Option<video::Background>,
    /// How `background` is scaled to fill the frame - "Cover" (crop to
    /// fill, the default) or "Contain" (letterbox/pillarbox to show all of
    /// it). Meaningless without a `background` set.
    background_fit: video::BackgroundFit,
    /// Opacity (0.0-1.0) of the black scrim blended over `background` so
    /// lyric text stays legible on top of it. Meaningless without a
    /// `background` set.
    background_dim: f32,
    /// Cached preview texture for `background`/`background_fit`/`color_bg`,
    /// alongside the values it was built from so the (fairly expensive -
    /// image decode, or an `ffmpeg` round trip for a video's poster frame)
    /// regeneration only happens when one of them actually changes, not
    /// every frame. `color_bg` is included because it's the letterbox/
    /// pillarbox pad color in "Contain" fit mode - without it, changing the
    /// background color picker would leave the preview showing the old pad
    /// color indefinitely (a real export would already be correct, since
    /// that's never cached - only this preview texture is). The inner
    /// `None` means "already tried and failed to load `background`" (e.g. a
    /// moved/deleted file) - remembered so a broken background doesn't
    /// retry that expensive load every single frame.
    background_preview: Option<(
        video::Background,
        video::BackgroundFit,
        egui::Color32,
        Option<egui::TextureHandle>,
    )>,
    /// User-configurable overrides for the word-pace/countdown-sensitivity
    /// heuristics in `lyrics.rs` - see [`lyrics::TimingSettings`]. Defaults
    /// to this app's own long-standing constants, so a project that never
    /// opens the "Timing" panel behaves exactly as before this setting
    /// existed.
    timing_settings: lyrics::TimingSettings,
    /// The system font family chosen for lyric text, if any - `None` means
    /// the bundled default (DejaVu Sans, the same font the video export
    /// always used before this existed). Applies to the video export and
    /// this app's own live preview only - never the `.cdg` export, whose
    /// font is a fixed 6x12-pixel bitmap format with no room for an
    /// arbitrary system font - see `fonts.rs`.
    selected_font_family: Option<String>,
    /// Every family name installed on this machine, once the background
    /// enumeration (kicked off at startup - see [`FontListJob`]) finishes.
    /// `None` while still loading; shown as "Loading fonts…" in the picker.
    font_list: Option<Vec<String>>,
    font_list_job: Option<FontListJob>,
    /// Loaded bytes for `selected_font_family`, alongside the name they
    /// were loaded for, so a change to `selected_font_family` is noticed
    /// and reloaded rather than silently reusing stale bytes. Cleared (not
    /// just left stale) whenever `selected_font_family` changes.
    font_bytes: Option<Vec<u8>>,
    /// Set when `selected_font_family` couldn't actually be loaded (e.g. a
    /// project referencing a font this machine doesn't have installed) -
    /// shown as a persistent notice near the font picker rather than only
    /// flashing by once in the status bar, and remembered so the (failed)
    /// load isn't retried every frame.
    font_load_error: Option<String>,
    /// Whichever font is currently registered with the egui context under
    /// [`LYRIC_FONT_FAMILY`] - `None` if none is (yet), so
    /// `ensure_egui_font_installed` only calls `Context::set_fonts` (not
    /// free - it rebuilds the font atlas) on an actual change, not every
    /// frame.
    installed_egui_font: Option<String>,
    /// Whether the font-picker's search/list panel is expanded.
    font_picker_open: bool,
    font_search: String,
    /// Whether a video export should mux in a vocals-reduced copy of the
    /// audio (via [`vocals::load_separator`]/[`vocals::separate`]) instead
    /// of the original.
    remove_vocals_for_video: bool,
    /// Whether the "Export…" options window is open.
    show_export_dialog: bool,
    /// Whether the in-app documentation ("Codex") window is open - see
    /// `codex.rs`/`draw_codex`.
    codex_open: bool,
    /// Index into [`codex::all_articles`] of the currently-shown article.
    codex_selected: usize,
    /// `egui_commonmark`'s own per-viewer cache (parsed markdown, loaded
    /// images) - kept across frames so re-rendering the same article every
    /// frame doesn't re-parse it from scratch each time. Shared with the
    /// first-run disclaimer modal below - it's just a rendering cache keyed
    /// by each `CommonMarkViewer`'s own id, not specific to the Codex.
    codex_cache: egui_commonmark::CommonMarkCache,
    /// Whether the first-run legal disclaimer modal (see `DISCLAIMER.md`
    /// and `draw_disclaimer_modal`) is currently showing - `true` only
    /// until acknowledged for the very first time on this machine (see
    /// `onboarding.rs`), never again after that, not tied to any project.
    show_disclaimer_modal: bool,
    export_cdg: bool,
    export_lrc: bool,
    export_ultrastar: bool,
    /// Karaoke subtitles (`.ass`, with a `\kf` word wipe) / plain
    /// subtitles (`.srt`). Session-only, like the other export picks.
    export_ass: bool,
    export_srt: bool,
    export_video: bool,
    export_instrumental: bool,
    /// Vocals-only stem, from the same separation pass as
    /// `export_instrumental` when both are requested together (see
    /// `start_combined_export`) - never a second, independently expensive
    /// model run.
    export_vocals: bool,
    /// Also package the `.cdg` with an `.mp3` into an MP3+G `.zip` (only
    /// when `.cdg` export is on). Session-only, like the other export picks.
    export_cdg_zip: bool,
    /// The status text as of last frame, its kind, and when it changed -
    /// see [`KaraokeApp::track_status`].
    status_seen: String,
    status_kind: StatusKind,
    /// Each timing-table row's on-screen vertical span and the table's
    /// visible area, as of last frame - for working out which row the
    /// mouse is over (whose action buttons then show).
    table_row_spans: Vec<egui::Rangef>,
    table_clip: egui::Rect,
    /// Frames since the saved UI size and the accent style were applied to
    /// the egui context (done on the first frame after `new()`, which has
    /// no context to apply them to). `None` = not applied yet.
    ui_style_frames: Option<u8>,
    /// The open "Copy timing" dialog, if any.
    copy_dialog: Option<CopyTimingDialog>,
    /// Whether the keyboard-shortcut cheat sheet (F1 / ?) is open.
    show_shortcuts: bool,
    /// In-progress text while the time readout is being edited to jump.
    time_jump_edit: Option<String>,
    /// While set, a toast for the current status shows until then.
    toast_until: Option<std::time::Instant>,
    /// Folder of the last export that wrote something - "Open folder".
    last_export_folder: Option<PathBuf>,
    /// Whether an export of the open project has succeeded since it was
    /// opened - the step bar's "Export" step.
    exported_this_project: bool,
    /// The running export (if any) is a batch export of other projects -
    /// so finishing it doesn't tick off the open project's Export step.
    batch_running: bool,
    /// Project files picked for a batch export in the export dialog.
    /// Session-only.
    batch_projects: Vec<PathBuf>,
    /// Whether the zip's `.mp3` is the instrumental (vocals removed) rather
    /// than the original song.
    zip_instrumental: bool,
    /// Whether an LRC export writes LRC2 word-level tags (from
    /// [`lyrics::word_timings`]) or plain line-level LRC1 - word-level is
    /// more precise but some older/simpler LRC readers only handle LRC1.
    lrc_enhanced_words: bool,
    /// Folder chosen (once) in the export dialog for all selected outputs.
    export_folder: Option<PathBuf>,
    /// Base filename (no extension) shared by all selected outputs -
    /// `{base}.cdg`, `{base}.lrc`, `{base}.txt` (UltraStar), `{base}.mp4`,
    /// `{base}-instrumental.{mp3,wav}`.
    export_base_name: String,
    instrumental_format: InstrumentalFormat,
    /// Set while a combined export (any mix of the outputs above) is
    /// running in a background thread.
    combined_export: Option<CombinedExportHandle>,

    /// Zoom/scroll state for the fine-tuning timeline.
    timeline_view: timeline::View,
    /// Set while a bubble's body or an edge handle is being dragged on the
    /// timeline - `None` the rest of the time.
    timeline_drag: Option<TimelineDrag>,
    /// Peaks for the currently loaded audio, once background extraction
    /// finishes - `None` before that, if extraction failed, or no audio is
    /// loaded. Drawn as a backdrop behind the timeline's bubbles.
    waveform: Option<waveform::Waveform>,
    /// Set while `waveform::build_waveform` is running in the background
    /// for the most recently loaded audio file.
    waveform_job: Option<WaveformJob>,

    /// Language model used for auto-align (see `align.rs`) - defaults to
    /// English since the app has no other language-awareness.
    align_language: align::AlignLanguage,
    /// Which vocal-isolation model auto-align uses to isolate vocals
    /// before running forced alignment against them (see
    /// `start_word_alignment`) - defaults to [`mdx::MdxModel::InstHq3`],
    /// same as everywhere else in the app. Session-only, like
    /// `align_language` itself - not saved with the project.
    align_vocal_model: mdx::MdxModel,
    /// Set while a background "Auto-align words" run is in progress.
    align_job: Option<AlignJob>,

    /// Snapshots to restore on Ctrl+Z, oldest first - see
    /// [`Self::track_undo_history`] for how/when these get pushed.
    undo_stack: VecDeque<UndoSnapshot>,
    /// Snapshots to restore on Ctrl+Shift+Z/Ctrl+Y, most-recently-undone
    /// last - cleared whenever a fresh (non-undo/redo) edit is recorded.
    redo_stack: VecDeque<UndoSnapshot>,
    /// The undoable state as of the end of the last frame - compared
    /// against the current state each frame to notice edits, so no
    /// individual tap/drag/nudge/parse call site needs to explicitly record
    /// undo history itself.
    undo_last_observed: UndoSnapshot,
    /// Set to the state from *before* the change currently under way, the
    /// first time a frame's state differs from `undo_last_observed` while
    /// no continuous gesture (a timeline drag, a focused text field) is in
    /// progress yet - so a multi-frame drag or a typing session ends up as
    /// one undo step instead of one per frame/keystroke. Finalized (pushed
    /// onto `undo_stack`) once the gesture ends.
    undo_pending_baseline: Option<UndoSnapshot>,
}

/// Which timeline bubble a [`TimelineDrag`] is dragging - a whole line, or
/// one word within the currently word-expanded line (see
/// [`KaraokeApp::word_tap_line`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimelineDragTarget {
    Line,
    Word(usize),
}

/// An in-progress drag on the fine-tuning timeline. Captured at
/// `drag_started()` and held for the duration of the drag so every frame
/// resolves the delta from the drag's *original* pointer position (rather
/// than compounding per-frame deltas, which would drift under egui's
/// rounding).
struct TimelineDrag {
    /// Index into `self.lines` of the line being dragged (whether the drag
    /// target is the line itself or one of its words).
    line_idx: usize,
    target: TimelineDragTarget,
    session: timeline::DragSession,
}

/// Audio format for the standalone instrumental export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum InstrumentalFormat {
    #[default]
    Mp3,
    Wav,
}

impl InstrumentalFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Mp3 => "MP3",
            Self::Wav => "WAV",
        }
    }
}

/// A named, curated set of all 12 colors, applied all at once - so a
/// first-time user gets a good-looking result without ever opening the
/// "Colors" section, and someone who does want to customize can start from
/// a preset closer to what they want instead of the single hardcoded
/// default. Picking one doesn't stick to future manual edits - it's a
/// one-time "set all these colors now", not a persistent mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum ColorPreset {
    #[default]
    Classic,
    Abyssal,
    HighContrast,
    Sunset,
    Ocean,
}

impl ColorPreset {
    fn label(self) -> &'static str {
        match self {
            Self::Classic => "Classic",
            Self::Abyssal => "Abyssal",
            Self::HighContrast => "High Contrast",
            Self::Sunset => "Sunset",
            Self::Ocean => "Ocean",
        }
    }

    fn palette(self) -> Palette {
        match self {
            Self::Classic => Palette::default(),
            Self::Abyssal => Palette::abyssal(),
            Self::HighContrast => Palette::high_contrast(),
            Self::Sunset => Palette::sunset(),
            Self::Ocean => Palette::ocean(),
        }
    }

    /// This preset's own bundled default lyric-text font, if it has one -
    /// applied once, alongside the colors, by `apply_color_preset` (see
    /// `fonts.rs`'s `BUNDLED_FONTS` docs for why "Nosifer" specifically is
    /// bundled rather than a system font lookup). `None` for every other
    /// preset: switching to one resets the font to the default only if the
    /// current font came from a preset, and otherwise leaves the user's own
    /// choice alone - the same "set it now, don't keep enforcing it"
    /// behavior the colors themselves already have.
    fn default_font(self) -> Option<&'static str> {
        match self {
            Self::Abyssal => Some("Nosifer"),
            Self::Classic | Self::HighContrast | Self::Sunset | Self::Ocean => None,
        }
    }
}
const ALL_COLOR_PRESETS: [ColorPreset; 5] = [
    ColorPreset::Classic,
    ColorPreset::Abyssal,
    ColorPreset::HighContrast,
    ColorPreset::Sunset,
    ColorPreset::Ocean,
];

/// Every user-pickable color, as one value - so an export can be built
/// from either the app's live colors or a saved project file's (batch
/// export) through the same conversions.
#[derive(Clone, Copy, Debug)]
struct ExportColors {
    bg: egui::Color32,
    default_unsung: egui::Color32,
    default_highlight: egui::Color32,
    male_unsung: egui::Color32,
    male_highlight: egui::Color32,
    female_unsung: egui::Color32,
    female_highlight: egui::Color32,
    duet_unsung: egui::Color32,
    duet_highlight: egui::Color32,
    preview: egui::Color32,
    title: egui::Color32,
    artist: egui::Color32,
    screaming_unsung: egui::Color32,
    screaming_highlight: egui::Color32,
    backing_unsung: egui::Color32,
    backing_highlight: egui::Color32,
}

impl ExportColors {
    /// A project file's colors, with the same fallbacks loading a project
    /// has always applied for files saved before a color existed.
    fn from_project(c: &project::ProjectColors) -> Self {
        let default_palette = Palette::default();
        Self {
            bg: color32_from_rgb_color(c.background),
            // A project saved before `Default` had its own color falls back
            // to `Male`'s - exactly how `Default` used to always render, so
            // an old project's look doesn't change just from being reopened.
            default_unsung: color32_from_rgb_color(c.default_unsung.unwrap_or(c.male_unsung)),
            default_highlight: color32_from_rgb_color(
                c.default_highlight.unwrap_or(c.male_highlight),
            ),
            male_unsung: color32_from_rgb_color(c.male_unsung),
            male_highlight: color32_from_rgb_color(c.male_highlight),
            female_unsung: color32_from_rgb_color(c.female_unsung),
            female_highlight: color32_from_rgb_color(c.female_highlight),
            duet_unsung: color32_from_rgb_color(c.duet_unsung),
            duet_highlight: color32_from_rgb_color(c.duet_highlight),
            preview: color32_from_rgb_color(c.preview),
            title: color32_from_rgb_color(c.title),
            artist: color32_from_rgb_color(c.artist),
            screaming_unsung: color32_from_rgb_color(c.screaming_unsung),
            screaming_highlight: color32_from_rgb_color(c.screaming_highlight),
            // A project saved before the Backing voice existed gets the
            // default palette's own backing colors.
            backing_unsung: c
                .backing_unsung
                .map(color32_from_rgb_color)
                .unwrap_or_else(|| color32_from_cdg(default_palette.backing_unsung)),
            backing_highlight: c
                .backing_highlight
                .map(color32_from_rgb_color)
                .unwrap_or_else(|| color32_from_cdg(default_palette.backing_highlight)),
        }
    }

    /// A voice's (upcoming, sung) colors.
    fn singer_pair(&self, s: Singer) -> (egui::Color32, egui::Color32) {
        match s {
            Singer::Default => (self.default_unsung, self.default_highlight),
            Singer::Male => (self.male_unsung, self.male_highlight),
            Singer::Female => (self.female_unsung, self.female_highlight),
            Singer::Duet => (self.duet_unsung, self.duet_highlight),
            Singer::Screaming => (self.screaming_unsung, self.screaming_highlight),
            Singer::Backing => (self.backing_unsung, self.backing_highlight),
        }
    }

    fn palette(&self) -> Palette {
        Palette {
            background: cdg_from_color32(self.bg),
            default_unsung: cdg_from_color32(self.default_unsung),
            default_highlight: cdg_from_color32(self.default_highlight),
            male_unsung: cdg_from_color32(self.male_unsung),
            male_highlight: cdg_from_color32(self.male_highlight),
            female_unsung: cdg_from_color32(self.female_unsung),
            female_highlight: cdg_from_color32(self.female_highlight),
            duet_unsung: cdg_from_color32(self.duet_unsung),
            duet_highlight: cdg_from_color32(self.duet_highlight),
            preview: cdg_from_color32(self.preview),
            title: cdg_from_color32(self.title),
            artist: cdg_from_color32(self.artist),
            screaming_unsung: cdg_from_color32(self.screaming_unsung),
            screaming_highlight: cdg_from_color32(self.screaming_highlight),
            backing_unsung: cdg_from_color32(self.backing_unsung),
            backing_highlight: cdg_from_color32(self.backing_highlight),
        }
    }

    fn video_palette(&self) -> VideoPalette {
        let c = |c: egui::Color32| video::Rgb8::new(c.r(), c.g(), c.b());
        VideoPalette {
            background: c(self.bg),
            default_unsung: c(self.default_unsung),
            default_highlight: c(self.default_highlight),
            male_unsung: c(self.male_unsung),
            male_highlight: c(self.male_highlight),
            female_unsung: c(self.female_unsung),
            female_highlight: c(self.female_highlight),
            duet_unsung: c(self.duet_unsung),
            duet_highlight: c(self.duet_highlight),
            preview: c(self.preview),
            title: c(self.title),
            artist: c(self.artist),
            screaming_unsung: c(self.screaming_unsung),
            screaming_highlight: c(self.screaming_highlight),
            backing_unsung: c(self.backing_unsung),
            backing_highlight: c(self.backing_highlight),
        }
    }
}

/// Which outputs to export and how - the export dialog's checkboxes,
/// shared by a normal export and every song in a batch export.
#[derive(Clone, Copy, Debug)]
struct ExportOptions {
    cdg: bool,
    lrc: bool,
    ultrastar: bool,
    ass: bool,
    srt: bool,
    video: bool,
    instrumental: bool,
    vocals: bool,
    /// MP3+G zip - only applies along with `cdg`.
    zip: bool,
    zip_instrumental: bool,
    lrc_enhanced: bool,
    remove_vocals_for_video: bool,
    instrumental_ext: &'static str,
}

impl ExportOptions {
    fn any(&self) -> bool {
        self.any_timed() || self.instrumental || self.vocals
    }

    /// Any output that uses lyric timing (as opposed to audio stems only).
    fn any_timed(&self) -> bool {
        self.cdg || self.video || self.lrc || self.ultrastar || self.ass || self.srt
    }

    /// Any output that needs the song's audio file.
    fn needs_audio(&self) -> bool {
        self.video || self.instrumental || self.vocals || (self.cdg && self.zip)
    }
}

/// Everything about one song an export needs - built from the open project
/// (`KaraokeApp::export_inputs`) or straight from a saved `.abyzl` file
/// ([`ExportInputs::from_project`], for batch export) without loading it
/// into the app.
struct ExportInputs {
    lines: Vec<LyricLine>,
    title: String,
    artist: String,
    colors: ExportColors,
    timing_settings: lyrics::TimingSettings,
    video_resolution: Resolution,
    video_codec: VideoCodec,
    video_style: video::VideoStyle,
    outro: lyrics::OutroSettings,
    show_credit_line: bool,
    background: Option<video::Background>,
    background_fit: video::BackgroundFit,
    background_dim: f32,
    font_family: Option<String>,
    font_bytes: Option<Vec<u8>>,
    /// The song's audio file, if it exists.
    audio_path: Option<PathBuf>,
    audio_duration: Option<f64>,
}

impl ExportInputs {
    /// A saved project's export inputs. Does file I/O (probing the audio
    /// file's length, loading the lyric font), so call it off the UI
    /// thread. Applies the same legacy migration loading a project does.
    fn from_project(mut p: project::ProjectFile) -> Self {
        lyrics::migrate_legacy_backing_vocals(&mut p.lines, &mut p.lyrics_raw);
        let audio_path = p.audio_path.filter(|a| a.is_file());
        let audio_duration = audio_path
            .as_deref()
            .and_then(|a| audio::probe_duration_secs(a).ok().flatten());
        let font_bytes = p
            .lyric_font_family
            .as_deref()
            .and_then(|f| fonts::load_family_bytes(f).ok());
        Self {
            colors: ExportColors::from_project(&p.colors),
            lines: p.lines,
            title: p.title,
            artist: p.artist,
            timing_settings: p.timing_settings,
            video_resolution: p.video_resolution,
            video_codec: p.video_codec,
            video_style: p.video_style,
            outro: p.outro,
            show_credit_line: p.show_credit_line,
            background: p.background,
            background_fit: p.background_fit,
            background_dim: p.background_dim,
            font_family: p.lyric_font_family,
            font_bytes,
            audio_path,
            audio_duration,
        }
    }

    /// Same as `KaraokeApp::resolved`: the main track, and the song's total
    /// length (audio length, or the last line's end if that's later).
    fn resolved(&self) -> (Vec<TimedLine>, f64) {
        let timed = lyrics::resolve_timing_with_settings(
            &self.lines,
            self.audio_duration,
            &self.timing_settings,
        );
        let backing_end = self
            .resolved_backing()
            .iter()
            .map(|b| b.sing_end)
            .fold(0.0, f64::max);
        let total = self
            .audio_duration
            .unwrap_or(0.0)
            .max(timed.last().map(|t| t.end).unwrap_or(0.0))
            .max(backing_end);
        (timed, total)
    }

    fn resolved_backing(&self) -> Vec<TimedLine> {
        lyrics::resolve_backing_track(&self.lines, &self.timing_settings)
    }
}

/// Why `lines` can't be exported yet, if every line isn't timed - a line
/// without a start would be silently left out of every timed output.
fn missing_timing(lines: &[LyricLine]) -> Option<String> {
    let total = lines.len();
    let timed_count = lines.iter().filter(|l| l.start.is_some()).count();
    if timed_count == 0 {
        Some("No lines are timed yet - tap along with the song first.".to_string())
    } else if timed_count < total {
        Some(format!(
            "{} of {total} line(s) don't have a timestamp yet - they'd be silently left \
             out of the output(s) otherwise. Tap along (or set times manually) for every \
             line before exporting.",
            total - timed_count
        ))
    } else {
        None
    }
}

/// Runs one song's export - every output `opts` selects, written to
/// `folder` as `base.<ext>` - reporting overall progress (0.0-1.0) through
/// `progress`. Blocking (vocal separation and video rendering can take
/// minutes), so run it on a background thread. The caller checks timing
/// and audio availability first.
fn run_export(
    inputs: &ExportInputs,
    opts: &ExportOptions,
    folder: &Path,
    base: &str,
    progress: &dyn Fn(f32),
) -> Vec<ExportOutcome> {
    let do_cdg = opts.cdg;
    let do_lrc = opts.lrc;
    let do_ultrastar = opts.ultrastar;
    let do_ass = opts.ass;
    let do_srt = opts.srt;
    let do_video = opts.video;
    let do_instrumental = opts.instrumental;
    let do_vocals = opts.vocals;
    let do_zip = do_cdg && opts.zip;
    let zip_instrumental = opts.zip_instrumental;
    let audio_path = inputs.audio_path.clone();

    let (timed, total_duration) = inputs.resolved();
    let backing = inputs.resolved_backing();
    // LRC has no notion of a second track, but it doesn't need one:
    // players just show whatever line's timestamp was reached most
    // recently, so backing lines are interleaved by start time.
    let mut lrc_lines: Vec<TimedLine> = timed.iter().chain(backing.iter()).cloned().collect();
    lrc_lines.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let timing_settings = inputs.timing_settings;
    let cdg_palette = inputs.colors.palette();
    let video_palette = inputs.colors.video_palette();
    let title = (!inputs.title.trim().is_empty()).then(|| inputs.title.trim().to_string());
    let artist = (!inputs.artist.trim().is_empty()).then(|| inputs.artist.trim().to_string());
    let resolution = inputs.video_resolution;
    let video_codec = inputs.video_codec;
    let video_style = inputs.video_style;
    let show_credit_line = inputs.show_credit_line;
    let background = inputs.background.clone();
    let background_fit = inputs.background_fit;
    let background_dim = inputs.background_dim;
    let custom_font_bytes = inputs.font_bytes.clone();
    let remove_vocals_for_video = opts.remove_vocals_for_video;
    let instrumental_ext = opts.instrumental_ext;
    let lrc_enhanced = opts.lrc_enhanced;
    let outro = export::resolve_outro(
        &inputs.outro,
        &timed,
        &backing,
        total_duration,
        title.is_some() || artist.is_some(),
    );

    let cdg_path = folder.join(format!("{base}.cdg"));
    let lrc_path = folder.join(format!("{base}.lrc"));
    let ultrastar_path = folder.join(format!("{base}.txt"));
    let ass_path = folder.join(format!("{base}.ass"));
    let srt_path = folder.join(format!("{base}.srt"));
    let subtitle_font = inputs
        .font_family
        .clone()
        .unwrap_or_else(|| "Arial".to_string());
    let video_path = folder.join(format!("{base}.mp4"));
    let instrumental_path = folder.join(format!("{base}-instrumental.{instrumental_ext}"));
    let vocals_path = folder.join(format!("{base}-vocals.{instrumental_ext}"));
    let zip_path = folder.join(format!("{base}.zip"));
    let zip_cdg_name = format!("{base}.cdg");
    let zip_mp3_name = format!("{base}.mp3");

    let mut outcomes = Vec::new();

    // Equal-weighted phases across whatever was selected - video
    // reports its own fine-grained sub-progress within its slice
    // (usually the slowest step by far), the others just jump from
    // 0% to 100% of their slice on completion.
    let phase_count = do_cdg as u32
        + do_lrc as u32
        + do_ultrastar as u32
        + do_ass as u32
        + do_srt as u32
        + do_video as u32
        + do_instrumental as u32
        + do_vocals as u32
        + do_zip as u32;
    let phase_count = phase_count.max(1);
    let mut phase_index = 0u32;
    let set_progress = |idx: u32, frac_within: f32| {
        let overall = (idx as f32 + frac_within.clamp(0.0, 1.0)) / phase_count as f32;
        progress(overall);
    };

    // Auto-pair the loaded audio next to whichever of .cdg/.lrc/
    // UltraStar's .txt actually gets written (same base filename,
    // in the same folder) so it's ready for pickup by a karaoke
    // player/game without a manual copy or rename - all of these
    // resolve to the *same* paired-audio destination for a given
    // base name/folder, so this only needs to run once even if
    // several of them are selected together.
    let mut paired_audio_sibling: Option<PathBuf> = None;
    // Kept for the MP3+G zip, which packages the exact same bytes.
    let mut cdg_bytes: Option<Vec<u8>> = None;

    if do_cdg {
        let bytes = export::render_cdg_with_settings(
            &timed,
            &backing,
            total_duration,
            &cdg_palette,
            title.as_deref(),
            artist.as_deref(),
            &timing_settings,
            outro.as_ref(),
        );
        let r = std::fs::write(&cdg_path, &bytes)
            .map(|()| cdg_path.clone())
            .map_err(|e| e.to_string());
        if do_zip {
            cdg_bytes = Some(bytes);
        }
        if r.is_ok() {
            paired_audio_sibling.get_or_insert_with(|| cdg_path.clone());
        }
        outcomes.push(ExportOutcome {
            label: "CDG file".into(),
            result: r,
        });

        phase_index += 1;
        set_progress(phase_index, 0.0);
    }

    if do_lrc {
        let text = formats::export_lrc(
            &lrc_lines,
            lrc_enhanced,
            title.as_deref(),
            artist.as_deref(),
        );
        let r = std::fs::write(&lrc_path, text)
            .map(|()| lrc_path.clone())
            .map_err(|e| e.to_string());
        if r.is_ok() {
            paired_audio_sibling.get_or_insert_with(|| lrc_path.clone());
        }
        outcomes.push(ExportOutcome {
            label: "LRC lyrics".into(),
            result: r,
        });

        phase_index += 1;
        set_progress(phase_index, 0.0);
    }

    if do_ultrastar {
        // UltraStar's `#MP3:` header is a bare filename relative to
        // the note file's own folder, not a path - it needs to
        // match whatever the paired-audio copy below actually ends
        // up named as.
        let mp3_filename = audio_path.as_ref().and_then(|a| {
            export::paired_audio_path(a, &ultrastar_path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
        });
        let text = formats::export_ultrastar(
            &timed,
            title.as_deref(),
            artist.as_deref(),
            mp3_filename.as_deref(),
        );
        let r = std::fs::write(&ultrastar_path, text)
            .map(|()| ultrastar_path.clone())
            .map_err(|e| e.to_string());
        if r.is_ok() {
            paired_audio_sibling.get_or_insert_with(|| ultrastar_path.clone());
        }
        outcomes.push(ExportOutcome {
            label: "UltraStar lyrics".into(),
            result: r,
        });

        phase_index += 1;
        set_progress(phase_index, 0.0);
    }

    if do_ass || do_srt {
        let card_end = export::title_card_end(&timed);
        if do_ass {
            let text = formats::export_ass(
                &timed,
                &backing,
                &video_palette,
                &subtitle_font,
                title.as_deref(),
                artist.as_deref(),
                card_end,
            );
            let mut text = text;
            if let Some(o) = &outro {
                formats::append_ass_outro(&mut text, o, title.as_deref(), artist.as_deref());
            }
            outcomes.push(ExportOutcome {
                label: "Karaoke subtitles (.ass)".into(),
                result: std::fs::write(&ass_path, text)
                    .map(|()| ass_path.clone())
                    .map_err(|e| e.to_string()),
            });
            phase_index += 1;
            set_progress(phase_index, 0.0);
        }
        if do_srt {
            let text =
                formats::export_srt(&lrc_lines, title.as_deref(), artist.as_deref(), card_end);
            let mut text = text;
            if let Some(o) = &outro {
                formats::append_srt_outro(&mut text, o, title.as_deref(), artist.as_deref());
            }
            outcomes.push(ExportOutcome {
                label: "Subtitles (.srt)".into(),
                result: std::fs::write(&srt_path, text)
                    .map(|()| srt_path.clone())
                    .map_err(|e| e.to_string()),
            });
            phase_index += 1;
            set_progress(phase_index, 0.0);
        }
    }

    if let Some(sibling) = &paired_audio_sibling {
        if let Some(audio_path) = &audio_path {
            let paired = export::copy_paired_audio(audio_path, sibling).map_err(|e| e.to_string());
            outcomes.push(ExportOutcome {
                label: "Paired audio".into(),
                result: paired,
            });
        }
    }

    // Vocal separation is shared: if the instrumental export, the
    // vocals export, and/or the video's "remove vocals" are all
    // requested together, run the (slow) separation model exactly
    // once - both stems come out of that single pass (see
    // `vocals.rs`), so there's never a reason to separate the same
    // song twice, no matter how many of these are picked.
    let need_separation =
        do_instrumental || do_vocals || remove_vocals_for_video || (do_zip && zip_instrumental);
    let shared_separation: Option<Result<vocals::Separation, String>> = if need_separation {
        audio_path.as_ref().map(|audio_path| {
            vocals::load_separator()
                .and_then(|mut sep| vocals::separate(&mut sep, audio_path, |_| {}))
                .map_err(|e| e.to_string())
        })
    } else {
        None
    };

    if do_instrumental {
        let r = match &shared_separation {
            Some(Ok(sep)) => {
                vocals::write_stem_to_file(&sep.instrumental, sep.sample_rate, &instrumental_path)
                    .map(|()| instrumental_path.clone())
                    .map_err(|e| e.to_string())
            }
            Some(Err(e)) => Err(e.clone()),
            None => Err("no audio loaded".to_string()),
        };
        outcomes.push(ExportOutcome {
            label: "Instrumental audio".into(),
            result: r,
        });
        phase_index += 1;
        set_progress(phase_index, 0.0);
    }

    if do_vocals {
        let r = match &shared_separation {
            Some(Ok(sep)) => vocals::write_stem_to_file(&sep.vocals, sep.sample_rate, &vocals_path)
                .map(|()| vocals_path.clone())
                .map_err(|e| e.to_string()),
            Some(Err(e)) => Err(e.clone()),
            None => Err("no audio loaded".to_string()),
        };
        outcomes.push(ExportOutcome {
            label: "Vocals audio".into(),
            result: r,
        });
        phase_index += 1;
        set_progress(phase_index, 0.0);
    }

    if do_zip {
        let zip_result: Result<PathBuf, String> = (|| {
            let cdg = cdg_bytes
                .as_ref()
                .ok_or_else(|| "the .cdg wasn't generated".to_string())?;
            let audio_path = audio_path
                .as_ref()
                .ok_or_else(|| "no audio loaded".to_string())?;
            let is_mp3 = audio_path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("mp3"));
            let tmp =
                std::env::temp_dir().join(format!("abyssal-cdg-zip-{}.mp3", std::process::id()));
            let mp3 = if zip_instrumental {
                match &shared_separation {
                    Some(Ok(sep)) => {
                        vocals::write_stem_to_file(&sep.instrumental, sep.sample_rate, &tmp)
                            .map_err(|e| format!("Vocal removal failed: {e}"))?;
                    }
                    Some(Err(e)) => return Err(format!("Vocal removal failed: {e}")),
                    None => return Err("no audio loaded".to_string()),
                }
                std::fs::read(&tmp).map_err(|e| e.to_string())
            } else if is_mp3 {
                std::fs::read(audio_path).map_err(|e| e.to_string())
            } else {
                vocals::encode_via_ffmpeg(audio_path, &tmp, "mp3")
                    .map_err(|e| format!("Converting the song to MP3 failed: {e}"))?;
                std::fs::read(&tmp).map_err(|e| e.to_string())
            };
            let _ = std::fs::remove_file(&tmp);
            let mp3 = mp3?;
            zipfile::write_zip(
                &zip_path,
                &[(zip_cdg_name.as_str(), cdg), (zip_mp3_name.as_str(), &mp3)],
            )
            .map(|()| zip_path.clone())
            .map_err(|e| e.to_string())
        })();
        outcomes.push(ExportOutcome {
            label: "MP3+G zip".into(),
            result: zip_result,
        });
        phase_index += 1;
        set_progress(phase_index, 0.0);
    }

    if do_video {
        let video_result: Result<PathBuf, String> = (|| {
            let audio_path = audio_path
                .as_ref()
                .ok_or_else(|| "no audio loaded".to_string())?;

            let mut own_temp: Option<PathBuf> = None;
            let render_audio_path: PathBuf = if remove_vocals_for_video {
                let tmp = std::env::temp_dir().join(format!(
                    "abyssal-cdg-instrumental-{}.wav",
                    std::process::id()
                ));
                match &shared_separation {
                    Some(Ok(sep)) => {
                        vocals::write_stem_to_file(&sep.instrumental, sep.sample_rate, &tmp)
                            .map_err(|e| format!("Vocal removal failed: {e}"))?;
                    }
                    Some(Err(e)) => return Err(format!("Vocal removal failed: {e}")),
                    None => return Err("Vocal removal failed: no audio loaded".to_string()),
                }
                own_temp = Some(tmp.clone());
                tmp
            } else {
                audio_path.clone()
            };

            let video_phase = phase_index;
            let r = video::render_video(
                &timed,
                &backing,
                total_duration,
                &video_palette,
                title.as_deref(),
                artist.as_deref(),
                resolution,
                video_codec,
                30,
                &render_audio_path,
                background.as_ref(),
                background_fit,
                background_dim,
                custom_font_bytes,
                &video_path,
                &timing_settings,
                show_credit_line,
                &video_style,
                outro.as_ref(),
                |p| set_progress(video_phase, p),
            );

            if let Some(tmp) = &own_temp {
                let _ = std::fs::remove_file(tmp);
            }

            r.map(|()| video_path.clone()).map_err(|e| e.to_string())
        })();

        outcomes.push(ExportOutcome {
            label: "Video".into(),
            result: video_result,
        });
        phase_index += 1;
        set_progress(phase_index, 0.0);
    }

    outcomes
}

/// One output's result from a combined export - a combined export can
/// partially succeed (e.g. ffmpeg missing but vocal separation working
/// fine), so each requested output is tracked and reported independently.
struct ExportOutcome {
    /// What was exported ("CDG file", ...) - prefixed with the song's name
    /// in a batch export.
    label: std::borrow::Cow<'static, str>,
    result: Result<PathBuf, String>,
}

/// Shared state for a combined export (any mix of .cdg/.mp4/instrumental
/// audio) running on a background thread, so the GUI stays responsive and
/// can show one progress bar spanning every selected output.
struct CombinedExportHandle {
    progress: Arc<AtomicU32>, // 0..=1000 (tenths of a percent), across all selected outputs
    result: Arc<Mutex<Option<Vec<ExportOutcome>>>>,
    /// Where the outputs are being written - offered as "Open folder" once done.
    folder: PathBuf,
}

/// A point-in-time copy of everything Ctrl+Z/Ctrl+Shift+Z can undo/redo -
/// the lyrics text, every line's timing/overrides, and the song's title and
/// artist. Deliberately narrower than a full `project::ProjectFile`: colors,
/// video resolution, and the loaded audio path are easy to redo by hand if
/// changed by mistake and aren't what "afraid to touch the timeline" is
/// about, so leaving them out keeps undo focused on the content that's
/// actually tedious to re-create (tapped/dragged timing).
#[derive(Clone, PartialEq)]
struct UndoSnapshot {
    lyrics_raw: String,
    lines: Vec<LyricLine>,
    title: String,
    artist: String,
}

/// Cap on how many undo steps are kept - generous for a single editing
/// session without letting the history grow unbounded.
const UNDO_HISTORY_LIMIT: usize = 100;

/// How far before a corrected timestamp `audition_seek` rewinds playback,
/// so you land a little before the edited point rather than exactly on it -
/// enough lead-in to hear the transition, not so much that you're waiting
/// through unrelated audio to get back to what you just changed.
const AUDITION_REWIND_SECS: f64 = 2.0;

/// The live preview's height (in points) at which its fixed pixel sizes
/// (countdown dots, small gaps) were tuned - the preview's old maximum size
/// (480x270). Above this, `draw_preview` scales them up proportionally.
const PREVIEW_BASE_HEIGHT: f32 = 270.0;

/// The 16:9 widths (in physical pixels) the live preview's background
/// texture can be rendered at - see `draw_preview`. Capped at 1080p, which
/// is more than any realistic preview panel/window needs.
const PREVIEW_BG_WIDTHS: [usize; 4] = [480, 960, 1440, 1920];

/// Key the custom lyric font is registered under in egui's font system -
/// both the `font_data` entry and the `FontFamily::Name` that references
/// it. Arbitrary but must be consistent between the two.
const LYRIC_FONT_KEY: &str = "lyric_custom_font";

/// Shared state for a background `waveform::build_waveform` call, kicked
/// off whenever a new audio file is loaded - decoding a whole song can take
/// a noticeable fraction of a second, so it happens off the UI thread the
/// same way exports do.
struct WaveformJob {
    result: Arc<Mutex<Option<Result<waveform::Waveform, String>>>>,
}

/// Shared state for the background `fonts::list_family_names` call, kicked
/// off once at startup - enumerating every font installed on the system can
/// take a noticeable moment, so it happens off the UI thread the same way
/// waveform decoding does, rather than freezing the first frame.
type FontListResult = Arc<Mutex<Option<Result<Vec<String>, String>>>>;

struct FontListJob {
    result: FontListResult,
}

/// One already-timed line's forced-alignment result, as reported back from
/// the background thread [`KaraokeApp::start_word_alignment`] spawns - see
/// [`AlignJob`].
struct AlignOutcome {
    line_idx: usize,
    result: Result<Vec<align::WordAlignment>, String>,
}

/// A whole-job-level `Err` for something that stopped an auto-align run
/// before it could even attempt any line (the alignment model failed to
/// load/download, no audio loaded), or `Ok` with one [`AlignOutcome`] per
/// line it tried (which can still individually fail without taking down
/// the rest of the run).
type AlignJobResult = Result<Vec<AlignOutcome>, String>;

/// Shared state for a background "auto-align words" run - see
/// [`AlignJobResult`].
struct AlignJob {
    progress: Arc<AtomicU32>, // 0..=1000 (tenths of a percent) across all lines being aligned
    result: Arc<Mutex<Option<AlignJobResult>>>,
}

impl KaraokeApp {
    fn new() -> Self {
        let (audio, audio_error) = match AudioPlayer::new() {
            Ok(a) => (Some(a), None),
            Err(e) => (
                None,
                Some(format!("Couldn't open an audio output device: {e}")),
            ),
        };
        let p = Palette::default();
        let mut app = Self {
            audio,
            audio_error,
            lyrics_raw: String::new(),
            lyrics_edited_span: None,
            current_project_path: None,
            pending_recovery: project::read_autosave(APP_ID),
            last_autosave: None,
            last_saved_snapshot: None,
            show_quit_confirm: false,
            quit_confirmed: false,
            pending_unsaved_action: None,
            lines: Vec::new(),
            next_untimed: 0,
            tap_phase: TapPhase::default(),
            word_tap_line: None,
            word_tap_mode: WordTapMode::default(),
            auto_follow_words: true,
            compact_timing_table: false,
            follow_timing_table: true,
            timing_table_scrolled_to: None,
            timing_table_scroll_request: None,
            prefs: prefs::load(APP_ID),
            shift_amount_secs: 0.1,
            loop_mode: None,
            loop_last_pos: None,
            preview_popped_out: false,
            preview_mode: PreviewMode::default(),
            cdg_preview: CdgPreviewState::default(),
            start_edit: None,
            end_edit: None,
            selected_lines: std::collections::BTreeSet::new(),
            selection_anchor: None,
            title: String::new(),
            artist: String::new(),
            color_bg: color32_from_cdg(p.background),
            color_default_unsung: color32_from_cdg(p.default_unsung),
            color_default_highlight: color32_from_cdg(p.default_highlight),
            color_male_unsung: color32_from_cdg(p.male_unsung),
            color_male_highlight: color32_from_cdg(p.male_highlight),
            color_female_unsung: color32_from_cdg(p.female_unsung),
            color_female_highlight: color32_from_cdg(p.female_highlight),
            color_duet_unsung: color32_from_cdg(p.duet_unsung),
            color_duet_highlight: color32_from_cdg(p.duet_highlight),
            color_preview: color32_from_cdg(p.preview),
            color_title: color32_from_cdg(p.title),
            color_artist: color32_from_cdg(p.artist),
            color_screaming_unsung: color32_from_cdg(p.screaming_unsung),
            color_screaming_highlight: color32_from_cdg(p.screaming_highlight),
            color_backing_unsung: color32_from_cdg(p.backing_unsung),
            color_backing_highlight: color32_from_cdg(p.backing_highlight),
            color_preset: ColorPreset::default(),
            recent_files: recent::load(APP_ID),
            status: String::new(),
            seek_drag_value: None,
            video_resolution: Resolution::Hd1080,
            video_codec: VideoCodec::default(),
            video_style: video::VideoStyle::default(),
            outro: lyrics::OutroSettings::default(),
            show_credit_line: true,
            background: None,
            background_fit: video::BackgroundFit::default(),
            background_dim: 0.4,
            background_preview: None,
            timing_settings: lyrics::TimingSettings::default(),
            selected_font_family: None,
            font_list: None,
            font_list_job: None,
            font_bytes: None,
            font_load_error: None,
            installed_egui_font: None,
            font_picker_open: false,
            font_search: String::new(),
            remove_vocals_for_video: false,
            show_export_dialog: false,
            codex_open: false,
            codex_selected: 0,
            codex_cache: egui_commonmark::CommonMarkCache::default(),
            show_disclaimer_modal: !onboarding::disclaimer_acknowledged(APP_ID),
            export_cdg: true,
            export_lrc: false,
            export_ultrastar: false,
            export_ass: false,
            export_srt: false,
            export_video: false,
            export_instrumental: false,
            export_vocals: false,
            export_cdg_zip: false,
            batch_projects: Vec::new(),
            exported_this_project: false,
            status_seen: String::new(),
            status_kind: StatusKind::Info,
            toast_until: None,
            time_jump_edit: None,
            show_shortcuts: false,
            copy_dialog: None,
            ui_style_frames: None,
            table_row_spans: Vec::new(),
            table_clip: egui::Rect::NOTHING,
            last_export_folder: None,
            batch_running: false,
            zip_instrumental: false,
            lrc_enhanced_words: true,
            export_folder: None,
            export_base_name: String::new(),
            instrumental_format: InstrumentalFormat::default(),
            combined_export: None,
            timeline_view: timeline::View::default(),
            timeline_drag: None,
            waveform: None,
            waveform_job: None,
            align_language: align::AlignLanguage::Eng,
            align_vocal_model: mdx::MdxModel::InstHq3,
            align_job: None,
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            undo_last_observed: UndoSnapshot {
                lyrics_raw: String::new(),
                lines: Vec::new(),
                title: String::new(),
                artist: String::new(),
            },
            undo_pending_baseline: None,
        };
        app.undo_last_observed = app.undo_snapshot();
        // The last export folder you chose, if it still exists.
        app.export_folder = app.prefs.export_folder.clone().filter(|d| d.is_dir());
        app.start_font_list_job();
        app
    }

    /// The voices the song actually uses (in the usual order) - just
    /// Default before any lines exist - so color checks skip unused ones.
    fn voices_in_use(&self) -> Vec<Singer> {
        let used: Vec<Singer> = Singer::ALL
            .into_iter()
            .filter(|s| self.lines.iter().any(|l| l.singer == *s))
            .collect();
        if used.is_empty() {
            vec![Singer::Default]
        } else {
            used
        }
    }

    /// The current colors, as one value (see [`ExportColors`]).
    fn export_colors(&self) -> ExportColors {
        ExportColors {
            bg: self.color_bg,
            default_unsung: self.color_default_unsung,
            default_highlight: self.color_default_highlight,
            male_unsung: self.color_male_unsung,
            male_highlight: self.color_male_highlight,
            female_unsung: self.color_female_unsung,
            female_highlight: self.color_female_highlight,
            duet_unsung: self.color_duet_unsung,
            duet_highlight: self.color_duet_highlight,
            preview: self.color_preview,
            title: self.color_title,
            artist: self.color_artist,
            screaming_unsung: self.color_screaming_unsung,
            screaming_highlight: self.color_screaming_highlight,
            backing_unsung: self.color_backing_unsung,
            backing_highlight: self.color_backing_highlight,
        }
    }

    fn palette(&self) -> Palette {
        self.export_colors().palette()
    }

    fn singer_colors(&self, s: Singer) -> (egui::Color32, egui::Color32) {
        match s {
            Singer::Default => (self.color_default_unsung, self.color_default_highlight),
            Singer::Male => (self.color_male_unsung, self.color_male_highlight),
            Singer::Female => (self.color_female_unsung, self.color_female_highlight),
            Singer::Duet => (self.color_duet_unsung, self.color_duet_highlight),
            Singer::Screaming => (self.color_screaming_unsung, self.color_screaming_highlight),
            Singer::Backing => (self.color_backing_unsung, self.color_backing_highlight),
        }
    }

    /// Every distinct custom singer name currently typed anywhere in this
    /// project, paired with the base [`Singer`] category it was first seen
    /// with - offered as one-click-reusable entries in each line's singer
    /// dropdown, so typing "Male 1" once on one line makes it selectable
    /// on every other line without retyping it. Sorted by name for a
    /// stable, predictable dropdown order.
    fn known_custom_singers(&self) -> Vec<(String, Singer)> {
        let mut seen: Vec<(String, Singer)> = Vec::new();
        for line in &self.lines {
            if let Some(name) = &line.custom_singer_name {
                if !seen.iter().any(|(n, _)| n == name) {
                    seen.push((name.clone(), line.singer));
                }
            }
        }
        seen.sort_by(|a, b| a.0.cmp(&b.0));
        seen
    }

    /// Sets every color at once from a named preset - see [`ColorPreset`].
    fn apply_color_preset(&mut self, preset: ColorPreset) {
        let p = preset.palette();
        self.color_bg = color32_from_cdg(p.background);
        self.color_default_unsung = color32_from_cdg(p.default_unsung);
        self.color_default_highlight = color32_from_cdg(p.default_highlight);
        self.color_male_unsung = color32_from_cdg(p.male_unsung);
        self.color_male_highlight = color32_from_cdg(p.male_highlight);
        self.color_female_unsung = color32_from_cdg(p.female_unsung);
        self.color_female_highlight = color32_from_cdg(p.female_highlight);
        self.color_duet_unsung = color32_from_cdg(p.duet_unsung);
        self.color_duet_highlight = color32_from_cdg(p.duet_highlight);
        self.color_preview = color32_from_cdg(p.preview);
        self.color_title = color32_from_cdg(p.title);
        self.color_artist = color32_from_cdg(p.artist);
        self.color_screaming_unsung = color32_from_cdg(p.screaming_unsung);
        self.color_screaming_highlight = color32_from_cdg(p.screaming_highlight);
        self.color_backing_unsung = color32_from_cdg(p.backing_unsung);
        self.color_backing_highlight = color32_from_cdg(p.backing_highlight);
        self.color_preset = preset;
        // A preset without its own font only resets the font if the current
        // one is some preset's bundled default (e.g. Nosifer left over from
        // Abyssal) - a font the user picked themselves is left alone. Checks
        // every preset rather than just the previous one, since loading a
        // project resets `color_preset` without touching the font.
        let current_is_preset_font = self.selected_font_family.as_deref().is_some_and(|f| {
            ALL_COLOR_PRESETS
                .iter()
                .any(|p| p.default_font() == Some(f))
        });
        let new_font = match preset.default_font() {
            Some(font_name) => Some(Some(font_name.to_string())),
            None if current_is_preset_font => Some(None),
            None => None,
        };
        if let Some(font) = new_font {
            self.selected_font_family = font;
            self.font_bytes = None;
            self.font_load_error = None;
        }
    }

    /// Resolve current lyric timing into a `(timed_lines, total_duration)`
    /// pair. Used by both export and the live preview so they always agree.
    fn resolved(&self) -> (Vec<TimedLine>, f64) {
        let duration_hint = self.audio.as_ref().and_then(|a| a.duration());
        let timed =
            lyrics::resolve_timing_with_settings(&self.lines, duration_hint, &self.timing_settings);
        let backing_end = self
            .resolved_backing()
            .iter()
            .map(|b| b.sing_end)
            .fold(0.0, f64::max);
        let total = duration_hint
            .unwrap_or(0.0)
            .max(timed.last().map(|t| t.end).unwrap_or(0.0))
            .max(backing_end);
        (timed, total)
    }

    /// The resolved backing track (see [`lyrics::resolve_backing_track`]) -
    /// every timed backing line, independent of the main track.
    fn resolved_backing(&self) -> Vec<TimedLine> {
        lyrics::resolve_backing_track(&self.lines, &self.timing_settings)
    }

    /// Snapshots everything a project file needs to capture from the
    /// current session - see `project.rs` for what's included and why.
    fn to_project_file(&self) -> project::ProjectFile {
        project::ProjectFile {
            version: project::CURRENT_VERSION,
            audio_path: self
                .audio
                .as_ref()
                .and_then(|a| a.path())
                .map(|p| p.to_path_buf()),
            lyrics_raw: self.lyrics_raw.clone(),
            lines: self.lines.clone(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            colors: project::ProjectColors {
                background: rgb_color_from_color32(self.color_bg),
                default_unsung: Some(rgb_color_from_color32(self.color_default_unsung)),
                default_highlight: Some(rgb_color_from_color32(self.color_default_highlight)),
                male_unsung: rgb_color_from_color32(self.color_male_unsung),
                male_highlight: rgb_color_from_color32(self.color_male_highlight),
                female_unsung: rgb_color_from_color32(self.color_female_unsung),
                female_highlight: rgb_color_from_color32(self.color_female_highlight),
                duet_unsung: rgb_color_from_color32(self.color_duet_unsung),
                duet_highlight: rgb_color_from_color32(self.color_duet_highlight),
                preview: rgb_color_from_color32(self.color_preview),
                title: rgb_color_from_color32(self.color_title),
                artist: rgb_color_from_color32(self.color_artist),
                screaming_unsung: rgb_color_from_color32(self.color_screaming_unsung),
                screaming_highlight: rgb_color_from_color32(self.color_screaming_highlight),
                backing_unsung: Some(rgb_color_from_color32(self.color_backing_unsung)),
                backing_highlight: Some(rgb_color_from_color32(self.color_backing_highlight)),
            },
            video_resolution: self.video_resolution,
            video_codec: self.video_codec,
            video_style: self.video_style,
            outro: self.outro.clone(),
            show_credit_line: self.show_credit_line,
            background: self.background.clone(),
            background_fit: self.background_fit,
            background_dim: self.background_dim,
            lyric_font_family: self.selected_font_family.clone(),
            timing_settings: self.timing_settings,
        }
    }

    /// Snapshots the subset of state Ctrl+Z/Ctrl+Shift+Z cover - see
    /// [`UndoSnapshot`].
    fn undo_snapshot(&self) -> UndoSnapshot {
        UndoSnapshot {
            lyrics_raw: self.lyrics_raw.clone(),
            lines: self.lines.clone(),
            title: self.title.clone(),
            artist: self.artist.clone(),
        }
    }

    /// Restores a previously-captured [`UndoSnapshot`] and resets whatever
    /// session-only state could otherwise point at something that no longer
    /// makes sense (a line that's gone, an in-progress edit on a line whose
    /// text just changed underneath it).
    fn apply_undo_snapshot(&mut self, snapshot: UndoSnapshot) {
        self.lyrics_raw = snapshot.lyrics_raw;
        self.lyrics_edited_span = None;
        self.lines = snapshot.lines;
        self.title = snapshot.title;
        self.artist = snapshot.artist;
        self.recompute_next_untimed();
        self.word_tap_line = None;
        self.timeline_drag = None;
        self.start_edit = None;
        self.end_edit = None;
        // Keep the "last observed" baseline in sync with what was just
        // restored, so next frame's `track_undo_history` doesn't mistake
        // this undo/redo itself for a fresh edit worth recording.
        self.undo_last_observed = self.undo_snapshot();
        self.undo_pending_baseline = None;
    }

    /// Pushes `snapshot` (the state from *before* an edit) onto the undo
    /// stack, capping its length, and clears the redo stack - a fresh edit
    /// invalidates whatever used to be ahead of it.
    fn push_undo_snapshot(&mut self, snapshot: UndoSnapshot) {
        self.undo_stack.push_back(snapshot);
        while self.undo_stack.len() > UNDO_HISTORY_LIMIT {
            self.undo_stack.pop_front();
        }
        self.redo_stack.clear();
    }

    /// Called once per frame (at the end of `update`) to notice edits and
    /// record undo history without needing every individual tap/drag/nudge
    /// call site to do it itself. `gesture_active` covers anything that
    /// spans multiple frames for one logical action - a timeline drag, or a
    /// focused text field being typed into - so that whole gesture
    /// coalesces into a single undo step once it ends, instead of one step
    /// per frame or per keystroke.
    fn track_undo_history(&mut self, gesture_active: bool) {
        let current = self.undo_snapshot();
        if current != self.undo_last_observed {
            if self.undo_pending_baseline.is_none() {
                self.undo_pending_baseline = Some(self.undo_last_observed.clone());
            }
            self.undo_last_observed = current;
        }
        if !gesture_active {
            if let Some(baseline) = self.undo_pending_baseline.take() {
                if baseline != self.undo_last_observed {
                    self.push_undo_snapshot(baseline);
                }
            }
        }
    }

    fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    fn undo(&mut self) {
        let Some(prev) = self.undo_stack.pop_back() else {
            self.status = "Nothing to undo.".to_string();
            return;
        };
        let current = self.undo_snapshot();
        self.redo_stack.push_back(current);
        self.apply_undo_snapshot(prev);
        self.status = "Undid last change.".to_string();
    }

    fn redo(&mut self) {
        let Some(next) = self.redo_stack.pop_back() else {
            self.status = "Nothing to redo.".to_string();
            return;
        };
        let current = self.undo_snapshot();
        self.undo_stack.push_back(current);
        self.apply_undo_snapshot(next);
        self.status = "Redid change.".to_string();
    }

    /// True if there's project content that isn't reflected in the last
    /// explicit save (or was never saved at all) - used both to decide
    /// whether the crash-recovery autosave should survive a clean exit
    /// (`on_exit`) and whether to intercept a window-close request with a
    /// "quit without saving?" prompt.
    fn has_unsaved_changes(&self) -> bool {
        let has_content =
            !self.lines.is_empty() || self.audio.as_ref().map(|a| a.is_loaded()).unwrap_or(false);
        has_content && self.last_saved_snapshot.as_ref() != Some(&self.to_project_file())
    }

    /// Replaces all project-level state (lyrics, timing, colors, title,
    /// resolution) with what's in `project`, and tries to load the audio
    /// file it references if the path still exists. Session-only state
    /// (timeline zoom, in-progress text-field edits, the fine-tune panel
    /// selection, ...) is reset rather than carried over, since none of it
    /// makes sense carried into a different project's content.
    fn apply_project_file(&mut self, project: project::ProjectFile) {
        self.lyrics_raw = project.lyrics_raw;
        self.lyrics_edited_span = None;
        self.lines = project.lines;
        // Projects saved with the old embedded "Echo" backing vocals get
        // them turned into independent backing-track lines (text included
        // in the lyrics box) - see `lyrics::migrate_legacy_backing_vocals`.
        let migrated_echoes =
            lyrics::migrate_legacy_backing_vocals(&mut self.lines, &mut self.lyrics_raw);
        self.title = project.title;
        self.artist = project.artist;
        self.video_resolution = project.video_resolution;
        self.video_codec = project.video_codec;
        self.video_style = project.video_style;
        self.outro = project.outro;
        self.show_credit_line = project.show_credit_line;
        self.background = project.background;
        self.background_fit = project.background_fit;
        self.background_dim = project.background_dim;
        self.background_preview = None;
        // A different (or absent) font than whatever was loaded before -
        // drop the cached bytes/error so `ensure_font_loaded` re-resolves
        // against the newly-loaded project's own selection next frame,
        // rather than keeping the previous project's font (or its failure).
        self.selected_font_family = project.lyric_font_family;
        self.font_bytes = None;
        self.font_load_error = None;
        self.timing_settings = project.timing_settings;

        let c = ExportColors::from_project(&project.colors);
        self.color_bg = c.bg;
        self.color_default_unsung = c.default_unsung;
        self.color_default_highlight = c.default_highlight;
        self.color_male_unsung = c.male_unsung;
        self.color_male_highlight = c.male_highlight;
        self.color_female_unsung = c.female_unsung;
        self.color_female_highlight = c.female_highlight;
        self.color_duet_unsung = c.duet_unsung;
        self.color_duet_highlight = c.duet_highlight;
        self.color_preview = c.preview;
        self.color_title = c.title;
        self.color_artist = c.artist;
        self.color_screaming_unsung = c.screaming_unsung;
        self.color_screaming_highlight = c.screaming_highlight;
        self.color_backing_unsung = c.backing_unsung;
        self.color_backing_highlight = c.backing_highlight;
        // A loaded project's colors may or may not match any preset - reset
        // to the neutral default label rather than keep showing whatever
        // preset happened to be selected before.
        self.color_preset = ColorPreset::default();

        self.recompute_next_untimed();
        self.word_tap_line = None;
        self.start_edit = None;
        self.end_edit = None;
        self.timeline_drag = None;
        self.timeline_view = timeline::View::default();
        self.seek_drag_value = None;

        // Undoing back into a *different* project's content would be
        // confusing (and could resurrect a previous project's lyrics after
        // opening an unrelated one), so a load/recovery starts a fresh undo
        // history rather than carrying the old one over.
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.undo_pending_baseline = None;
        self.undo_last_observed = self.undo_snapshot();

        // Cleared unconditionally (rather than left for `load_audio` to
        // replace) so a project with no saved audio path doesn't keep
        // showing whatever song's waveform happened to be loaded before.
        self.waveform = None;
        self.waveform_job = None;

        // The "Export…" dialog's own "what to export, and how" choices
        // aren't part of `project::ProjectFile` at all (they're this
        // session's export intent, not the song's own data) - reset to
        // the same fresh defaults `KaraokeApp::new()` starts with rather
        // than silently carrying over whatever the *previous* project had
        // selected, which otherwise looks like the newly loaded project's
        // own settings didn't take effect. `export_folder` is left alone
        // deliberately - a remembered output location is a genuine
        // convenience, not something tied to any one project's identity.
        self.export_cdg = true;
        self.export_lrc = false;
        self.export_ultrastar = false;
        self.export_ass = false;
        self.export_srt = false;
        self.export_video = false;
        self.export_instrumental = false;
        self.export_vocals = false;
        self.export_cdg_zip = false;
        self.zip_instrumental = false;
        self.exported_this_project = false;
        self.last_export_folder = None;
        self.lrc_enhanced_words = true;
        self.remove_vocals_for_video = false;
        self.instrumental_format = InstrumentalFormat::default();
        // Emptied rather than left as whatever the previous project's name
        // resolved to - `open_export_dialog` only re-derives this "if it's
        // still empty", so a non-empty leftover would otherwise silently
        // keep the old project's filename forever.
        self.export_base_name.clear();

        match project.audio_path {
            Some(path) if path.exists() => {
                self.load_audio(path);
                self.status = format!("Loaded project - {}", self.status);
            }
            Some(path) => {
                self.status = format!(
                    "Loaded project, but its audio file wasn't found at {} - load it manually.",
                    path.display()
                );
            }
            None => {
                self.status = "Loaded project (no audio file was saved with it).".to_string();
            }
        }
        if migrated_echoes > 0 {
            self.status = format!(
                "{} Converted {migrated_echoes} old echo vocal(s) into backing-vocal lines.",
                self.status
            );
        }
    }

    fn project_busy(&self) -> bool {
        self.combined_export.is_some() || self.align_job.is_some()
    }

    fn default_project_file_name(&self) -> String {
        let stem = self
            .audio
            .as_ref()
            .and_then(|a| a.file_name())
            .map(|n| n.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or(n))
            .filter(|s| !s.is_empty())
            .or_else(|| {
                let t = self.title.trim();
                (!t.is_empty()).then(|| t.to_string())
            })
            .unwrap_or_else(|| "karaoke-project".to_string());
        format!("{stem}.{}", project::FILE_EXTENSION)
    }

    /// Ctrl+S / "Save Project" - writes straight back to
    /// `current_project_path` if there is one, else falls back to the
    /// "Save Project As…" dialog (there's nowhere to write to yet).
    fn save_project(&mut self) {
        if let Some(path) = self.current_project_path.clone() {
            self.write_project_to(&path);
        } else {
            self.save_project_as_dialog();
        }
    }

    fn save_project_as_dialog(&mut self) {
        let default_name = self.default_project_file_name();
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name(&default_name)
            .add_filter("Abyssal CDG project", &[project::FILE_EXTENSION])
            .save_file()
        {
            let path = project::ensure_project_extension(path);
            self.write_project_to(&path);
        }
    }

    fn write_project_to(&mut self, path: &Path) {
        let project = self.to_project_file();
        match project.save_to_file(path) {
            Ok(()) => {
                self.current_project_path = Some(path.to_path_buf());
                self.last_saved_snapshot = Some(project);
                self.status = format!("Saved project to {}", path.display());
                recent::record_project(APP_ID, path);
                self.recent_files = recent::load(APP_ID);
            }
            Err(e) => {
                self.status = format!("Couldn't save project: {e}");
            }
        }
    }

    fn load_project_dialog(&mut self) {
        if self.project_busy() {
            self.status = "Can't load a project while an export is running.".to_string();
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Abyssal CDG project", &[project::FILE_EXTENSION])
            .pick_file()
        {
            self.request_project_load(path);
        }
    }

    /// Entry point for all three ways a project load can be triggered (the
    /// file picker above, the Recent menu, dragging a `.abyzl` file onto
    /// the window) - loads immediately if there's nothing at risk, or asks
    /// first via `draw_unsaved_action_confirm_prompt` if there are unsaved
    /// changes to the currently open project, instead of silently
    /// discarding them.
    fn request_project_load(&mut self, path: PathBuf) {
        if self.has_unsaved_changes() {
            self.pending_unsaved_action = Some(PendingUnsavedAction::LoadProject(path));
        } else {
            self.load_project_file(path);
        }
    }

    /// Entry point for "New Project" - same unsaved-changes protection as
    /// `request_project_load`, above.
    fn request_new_project(&mut self) {
        if self.project_busy() {
            self.status = "Can't start a new project while an export is running.".to_string();
            return;
        }
        if self.has_unsaved_changes() {
            self.pending_unsaved_action = Some(PendingUnsavedAction::NewProject);
        } else {
            self.start_new_project();
        }
    }

    /// Resets every bit of state back to a freshly launched app's own
    /// defaults - deliberately `*self = KaraokeApp::new()` rather than a
    /// hand-maintained list of fields to reset: a hand-maintained list is
    /// exactly the class of bug that made the export dialog's own settings
    /// silently carry over between projects before `apply_project_file`
    /// was fixed to reset them - reconstructing the whole struct the same
    /// way startup does can't have the same kind of gap, by construction.
    ///
    /// The autosave slot is cleared first, specifically so `KaraokeApp::new`
    /// doesn't immediately find its own leftover autosave and mistake it
    /// for a crash to recover from: by the time this runs, the caller
    /// (`request_new_project`) has already established that whatever was
    /// open is either fully saved or explicitly OK to discard, so there's
    /// nothing left that still needs recovering. Without this, the
    /// still-on-disk autosave from the periodic 30s timer (never cleared by
    /// "Save Project" itself, which writes to a completely different file)
    /// would trigger the "Recover previous session?" prompt on every New
    /// Project, even right after an explicit save.
    fn start_new_project(&mut self) {
        project::clear_autosave(APP_ID);
        *self = KaraokeApp::new();
    }

    fn load_project_file(&mut self, path: PathBuf) {
        match project::ProjectFile::load_from_file(&path) {
            Ok(loaded) => {
                self.apply_project_file(loaded.clone());
                recent::record_project(APP_ID, &path);
                self.recent_files = recent::load(APP_ID);
                self.current_project_path = Some(path);
                // The live state now matches what's on disk - not "unsaved
                // work" from `on_exit`'s point of view.
                self.last_saved_snapshot = Some(loaded);
            }
            Err(e) => {
                self.status = format!("Couldn't load project {}: {e}", path.display());
            }
        }
    }

    /// Dropdown of recently opened/saved project and audio files - lets a
    /// returning user pick up a previous session without hunting through a
    /// file picker for something they already located once. Entries are
    /// cloned out of `self.recent_files` before iterating so clicking one
    /// (which needs `&mut self`) doesn't fight the borrow checker.
    fn draw_recent_files_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("🕘 Recent", |ui| {
            let file_label = |path: &Path| {
                path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string())
            };
            if self.recent_files.projects.is_empty() && self.recent_files.audio.is_empty() {
                ui.label(egui::RichText::new("(nothing yet)").weak());
                return;
            }
            if !self.recent_files.projects.is_empty() {
                ui.label(egui::RichText::new("Projects").small().weak());
                for path in self.recent_files.projects.clone() {
                    if ui.button(file_label(&path)).clicked() {
                        ui.close_menu();
                        if !path.exists() {
                            self.status = format!("\"{}\" no longer exists.", file_label(&path));
                        } else if self.project_busy() {
                            self.status =
                                "Can't load a project while an export is running.".to_string();
                        } else {
                            self.request_project_load(path);
                        }
                    }
                }
            }
            if !self.recent_files.audio.is_empty() {
                if !self.recent_files.projects.is_empty() {
                    ui.separator();
                }
                ui.label(egui::RichText::new("Audio").small().weak());
                for path in self.recent_files.audio.clone() {
                    if ui.button(file_label(&path)).clicked() {
                        ui.close_menu();
                        if !path.exists() {
                            self.status = format!("\"{}\" no longer exists.", file_label(&path));
                        } else {
                            self.load_audio(path);
                        }
                    }
                }
            }
        });
    }

    /// Draws the full-window "recover previous session?" prompt shown when
    /// `pending_recovery` is set, in place of the rest of the UI until the
    /// user decides - see `AUTOSAVE_INTERVAL`/`on_exit` for how an autosave
    /// does (or doesn't) end up there in the first place.
    fn draw_recovery_prompt(&mut self, ctx: &egui::Context) {
        let Some(recovered) = self.pending_recovery.clone() else {
            return;
        };
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                ui.heading("Recover previous session?");
                ui.add_space(8.0);
                ui.scope(|ui| {
                    ui.set_max_width(480.0);
                    ui.label(
                        "There's autosaved work from your last session that was never \
                         explicitly saved - whether that's because the app closed \
                         unexpectedly (a crash, a force-quit, a system shutdown) or it was \
                         just closed before you got a chance to save.",
                    );
                });
                ui.add_space(12.0);

                ui.group(|ui| {
                    let timed_count = recovered.lines.iter().filter(|l| l.start.is_some()).count();
                    if !recovered.title.trim().is_empty() {
                        ui.label(format!("Title: {}", recovered.title));
                    }
                    if !recovered.artist.trim().is_empty() {
                        ui.label(format!("Artist: {}", recovered.artist));
                    }
                    ui.label(format!(
                        "{} line(s), {} timed",
                        recovered.lines.len(),
                        timed_count
                    ));
                    if let Some(path) = &recovered.audio_path {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.display().to_string());
                        ui.label(format!("Audio: {name}"));
                    }
                });

                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    ui.add_space(ui.available_width() / 2.0 - 90.0);
                    if ui.button("Recover").clicked() {
                        self.apply_project_file(recovered.clone());
                        self.status = "Recovered your previous session.".to_string();
                        self.pending_recovery = None;
                        project::clear_autosave(APP_ID);
                    }
                    if ui.button("Discard").clicked() {
                        self.pending_recovery = None;
                        project::clear_autosave(APP_ID);
                    }
                });
            });
        });
    }

    /// Draws the "quit without saving?" prompt shown when a window-close
    /// request was intercepted because of unsaved changes - see the
    /// `close_requested()` check in `update`.
    fn draw_quit_confirm_prompt(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                ui.heading("Quit without saving?");
                ui.add_space(8.0);
                ui.scope(|ui| {
                    ui.set_max_width(480.0);
                    ui.label(
                        "You have changes that haven't been saved to a project file yet. \
                         They're protected by autosave and can be recovered next time you \
                         open the app, but you can also save for real right now.",
                    );
                });
                ui.add_space(16.0);

                ui.horizontal(|ui| {
                    ui.add_space(ui.available_width() / 2.0 - 150.0);
                    if ui.button("Save and Quit").clicked() {
                        self.save_project();
                        if !self.has_unsaved_changes() {
                            self.show_quit_confirm = false;
                            self.quit_confirmed = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        // Else the save was cancelled (e.g. the "Save
                        // Project As…" dialog was dismissed) or failed -
                        // stay on this screen rather than quitting anyway.
                    }
                    if ui.button("Quit Without Saving").clicked() {
                        self.show_quit_confirm = false;
                        self.quit_confirmed = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("Cancel").clicked() {
                        self.show_quit_confirm = false;
                    }
                });
            });
        });
    }

    /// Shown (in place of the rest of the UI, like `draw_quit_confirm_prompt`)
    /// while `pending_unsaved_action` is `Some` - see `request_project_load`/
    /// `request_new_project`. Same three-button shape either way (save
    /// first / discard and continue / cancel), just with action-specific
    /// wording so it's clear what's actually about to happen.
    fn draw_unsaved_action_confirm_prompt(&mut self, ctx: &egui::Context) {
        let (heading, body, save_label, discard_label) = match self.pending_unsaved_action {
            Some(PendingUnsavedAction::NewProject) => (
                "Start a new project without saving?",
                "You have changes that haven't been saved to a project file yet. Starting a \
                 new project will replace them - they're protected by autosave and can be \
                 recovered next time you open the app, but you can also save for real right \
                 now.",
                "Save and Start New",
                "Start New Without Saving",
            ),
            Some(PendingUnsavedAction::LoadProject(_)) => (
                "Load without saving?",
                "You have changes that haven't been saved to a project file yet. Loading a \
                 different project will replace them - they're protected by autosave and can \
                 be recovered next time you open the app, but you can also save for real right \
                 now.",
                "Save and Load",
                "Load Without Saving",
            ),
            None => return,
        };
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                ui.heading(heading);
                ui.add_space(8.0);
                ui.scope(|ui| {
                    ui.set_max_width(480.0);
                    ui.label(body);
                });
                ui.add_space(16.0);

                ui.horizontal(|ui| {
                    ui.add_space(ui.available_width() / 2.0 - 150.0);
                    if ui.button(save_label).clicked() {
                        self.save_project();
                        if !self.has_unsaved_changes() {
                            if let Some(action) = self.pending_unsaved_action.take() {
                                self.run_pending_unsaved_action(action);
                            }
                        }
                        // Else the save was cancelled (e.g. the "Save
                        // Project As…" dialog was dismissed) or failed -
                        // stay on this screen rather than continuing anyway.
                    }
                    if ui.button(discard_label).clicked() {
                        if let Some(action) = self.pending_unsaved_action.take() {
                            self.run_pending_unsaved_action(action);
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.pending_unsaved_action = None;
                    }
                });
            });
        });
    }

    fn run_pending_unsaved_action(&mut self, action: PendingUnsavedAction) {
        match action {
            PendingUnsavedAction::NewProject => self.start_new_project(),
            PendingUnsavedAction::LoadProject(path) => self.load_project_file(path),
        }
    }

    fn load_audio_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Audio", &["mp3", "wav", "flac", "ogg", "m4a", "aac"])
            .pick_file()
        {
            self.load_audio(path);
        }
    }

    fn load_audio(&mut self, path: PathBuf) {
        if let Some(audio) = &mut self.audio {
            match audio.load(path.clone()) {
                Ok(()) => {
                    self.status = "Audio loaded.".to_string();
                    // Only guess from the filename when both fields are
                    // still blank - never overwrite a title/artist the user
                    // (or a loaded project) already set.
                    if self.title.trim().is_empty() && self.artist.trim().is_empty() {
                        if let Some((artist, title)) = parse_artist_title_from_filename(&path) {
                            self.artist = artist;
                            self.title = title;
                            self.status =
                                "Audio loaded. Title/artist auto-filled from the filename."
                                    .to_string();
                        }
                    }
                    recent::record_audio(APP_ID, &path);
                    self.recent_files = recent::load(APP_ID);
                    self.start_waveform_job(path);
                }
                Err(e) => {
                    self.status = format!("Couldn't load audio: {e}");
                }
            }
        }
    }

    /// Lets the user pick an image or video to show behind the lyrics in
    /// the video export/preview (see [`video::Background`]) - one dialog,
    /// filtered to both kinds of file, with the kind itself guessed from
    /// the extension on pick.
    fn choose_background_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Image or video",
                &[
                    "png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff", "mp4", "mov", "mkv",
                    "avi", "webm", "m4v",
                ],
            )
            .pick_file()
        {
            match video::Background::from_path(path) {
                Some(bg) => {
                    self.status = format!("Background set to {}.", bg.path().display());
                    self.background = Some(bg);
                    self.background_preview = None;
                }
                None => {
                    self.status = "That file isn't a supported image or video format.".to_string();
                }
            }
        }
    }

    /// Rebuilds the live preview's background texture if it's stale (i.e.
    /// doesn't match `self.background` anymore, or was rendered at a
    /// different `w`x`h` than now requested). A no-op most frames -
    /// decoding an image or extracting a video's poster frame only happens
    /// once per background selection, not once per frame. A failed load
    /// (a moved/deleted file, an unreadable video, ...) is cached too - as
    /// "no texture" - so it isn't retried every frame; the failure reason
    /// is shown once in the status bar instead.
    fn ensure_background_preview(&mut self, ctx: &egui::Context, w: usize, h: usize) {
        let Some(bg) = self.background.clone() else {
            self.background_preview = None;
            return;
        };
        let fit = self.background_fit;
        let color = self.color_bg;
        // A failed load (`None` texture) counts as matching any size, so a
        // panel resize doesn't retry it.
        if matches!(&self.background_preview, Some((cached, cached_fit, cached_color, tex)) if *cached == bg && *cached_fit == fit && *cached_color == color && tex.as_ref().is_none_or(|t| t.size() == [w, h]))
        {
            return;
        }
        let pad_color = video::Rgb8::new(color.r(), color.g(), color.b());
        let rgb = match &bg {
            video::Background::Image(path) => {
                video::load_image_background(path, w as u32, h as u32, fit, pad_color)
            }
            video::Background::Video(path) => {
                video::extract_video_background_thumbnail(path, w as u32, h as u32, fit, pad_color)
            }
        };
        match rgb {
            Ok(bytes) => {
                let image = egui::ColorImage::from_rgb([w, h], &bytes);
                let texture =
                    ctx.load_texture("background_preview", image, egui::TextureOptions::LINEAR);
                self.background_preview = Some((bg, fit, color, Some(texture)));
            }
            Err(e) => {
                self.status = format!("Couldn't load a background preview: {e:#}");
                self.background_preview = Some((bg, fit, color, None));
            }
        }
    }

    /// Kicks off waveform-peak extraction for `path` on a background
    /// thread - see [`WaveformJob`]. Replaces any previous waveform/job
    /// immediately, so switching audio files never briefly shows the old
    /// file's waveform under the new one's bubbles.
    fn start_waveform_job(&mut self, path: PathBuf) {
        self.waveform = None;
        let result: Arc<Mutex<Option<Result<waveform::Waveform, String>>>> =
            Arc::new(Mutex::new(None));
        let result_clone = result.clone();
        self.waveform_job = Some(WaveformJob { result });
        std::thread::spawn(move || {
            let outcome = waveform::build_waveform(&path).map_err(|e| e.to_string());
            *result_clone.lock().unwrap() = Some(outcome);
        });
    }

    /// Checks on a running waveform job, if any. Returns true while still
    /// in progress (so the caller knows to keep repainting). A failed
    /// extraction is silently dropped - the timeline works exactly the
    /// same without a waveform, just without the extra visual cue.
    fn poll_waveform_job(&mut self) -> bool {
        let Some(job) = &self.waveform_job else {
            return false;
        };
        let finished = job.result.lock().unwrap().take();
        match finished {
            Some(outcome) => {
                self.waveform = outcome.ok();
                self.waveform_job = None;
                false
            }
            None => true,
        }
    }

    /// Kicks off the one-time background system-font enumeration - see
    /// [`FontListJob`]. Called once, from [`Self::new`].
    fn start_font_list_job(&mut self) {
        let result: FontListResult = Arc::new(Mutex::new(None));
        let result_clone = result.clone();
        self.font_list_job = Some(FontListJob { result });
        std::thread::spawn(move || {
            let outcome = fonts::list_family_names().map_err(|e| e.to_string());
            *result_clone.lock().unwrap() = Some(outcome);
        });
    }

    /// Checks on the font-enumeration job, if still running. A failure
    /// (e.g. no working font backend on this system) just leaves the
    /// picker showing "no fonts found" rather than blocking anything -
    /// custom fonts are optional, the bundled default still works.
    fn poll_font_list_job(&mut self) -> bool {
        let Some(job) = &self.font_list_job else {
            return false;
        };
        let finished = job.result.lock().unwrap().take();
        match finished {
            Some(outcome) => {
                self.font_list = Some(outcome.unwrap_or_default());
                self.font_list_job = None;
                false
            }
            None => true,
        }
    }

    /// Loads `selected_font_family`'s bytes into `font_bytes` if it isn't
    /// already cached for the current selection - called every frame
    /// (cheap: a name comparison in the common case where nothing changed),
    /// so a change made through the font picker takes effect immediately.
    /// Reading a font file is fast enough on a local disk that this runs
    /// synchronously rather than as its own background job, the same way
    /// `ensure_background_preview` loads a background image/video's
    /// thumbnail synchronously.
    fn ensure_font_loaded(&mut self) {
        match &self.selected_font_family {
            None => {
                self.font_bytes = None;
                self.font_load_error = None;
            }
            Some(name) => {
                // Already loaded for this exact selection, or already
                // failed for it - nothing to do either way.
                if self.font_bytes.is_some() || self.font_load_error.is_some() {
                    return;
                }
                match fonts::load_family_bytes(name) {
                    Ok(bytes) => {
                        self.font_bytes = Some(bytes);
                        self.font_load_error = None;
                    }
                    Err(e) => {
                        let msg = format!("Font \"{name}\" not found, using default. ({e:#})");
                        // Also surface this in the status bar - the Font
                        // panel is collapsed by default, so a project that
                        // references a missing font (e.g. opened on a
                        // different machine) shouldn't fail silently just
                        // because nobody happened to expand that panel.
                        self.status = msg.clone();
                        self.font_load_error = Some(msg);
                    }
                }
            }
        }
    }

    /// The bytes to actually use for lyric text right now - the loaded
    /// custom font, or `None` for the bundled default. A cheap clone
    /// (called once per video export, not per frame).
    fn loaded_font_bytes(&self) -> Option<Vec<u8>> {
        self.font_bytes.clone()
    }

    /// Registers (or un-registers) the custom lyric font with egui's own
    /// font system under [`LYRIC_FONT_FAMILY`], only when the effective
    /// selection has actually changed since the last call - `Context::
    /// set_fonts` rebuilds the font atlas, not something to do every frame.
    /// The rest of the UI (buttons, labels, ...) is untouched either way;
    /// only widgets that explicitly ask for `LYRIC_FONT_FAMILY` (the live
    /// preview's lyric text) are affected.
    fn ensure_egui_font_installed(&mut self, ctx: &egui::Context) {
        let target_name = if self.font_bytes.is_some() {
            self.selected_font_family.clone()
        } else {
            None
        };
        if self.installed_egui_font == target_name {
            return;
        }
        let mut defs = egui::FontDefinitions::default();
        if let Some(bytes) = &self.font_bytes {
            defs.font_data.insert(
                LYRIC_FONT_KEY.to_string(),
                egui::FontData::from_owned(bytes.clone()),
            );
            defs.families.insert(
                egui::FontFamily::Name(LYRIC_FONT_KEY.into()),
                vec![LYRIC_FONT_KEY.to_string()],
            );
        }
        ctx.set_fonts(defs);
        self.installed_egui_font = target_name;
    }

    /// The font family the live preview's lyric text should draw with -
    /// the custom font if one's active and actually loaded, otherwise
    /// egui's normal default (matches every other label in the app).
    fn lyric_font_family(&self) -> egui::FontFamily {
        if self.installed_egui_font.is_some() {
            egui::FontFamily::Name(LYRIC_FONT_KEY.into())
        } else {
            egui::FontFamily::Proportional
        }
    }

    /// Kicks off a background "auto-align words" run: one `align::align_line`
    /// call per already-timed line with more than one word, each restricted
    /// to that line's own tapped window (see `align.rs` for why per-line
    /// rather than one whole-song call). Populates every word's start *and*
    /// end from the result, overwriting any existing manual/estimated word
    /// timing for lines it successfully aligns - Ctrl+Z undoes the whole
    /// run in one step if the result isn't an improvement.
    fn start_word_alignment(&mut self) {
        if self.project_busy() {
            self.status = "Can't auto-align while another operation is running.".to_string();
            return;
        }
        let Some(audio_path) = self.audio.as_ref().and_then(|a| a.path()) else {
            self.status = "Load audio first - auto-align needs it.".to_string();
            return;
        };
        let audio_path = audio_path.to_path_buf();

        let (timed, indices) = self.resolved_with_indices();
        let total_duration = self.resolved().1;
        let mut requests: Vec<(usize, Vec<String>, f64, f64)> = Vec::new();
        for (i, timed_line) in timed.iter().enumerate() {
            let orig_idx = indices[i];
            // Whole words, syllable marks removed - the speech model aligns
            // words; each word's time is spread over its syllables after
            // (see `lyrics::spread_word_timing` in `poll_align_job`).
            let words: Vec<String> = lyrics::normalize_text(&self.lines[orig_idx].text)
                .split(' ')
                .filter(|w| !w.is_empty())
                .map(|w| w.to_string())
                .collect();
            if words.len() < 2 {
                continue; // nothing to split within a single-word line
            }
            let prev_bound = if i > 0 { timed[i - 1].sing_end } else { 0.0 };
            let next_bound = timed.get(i + 1).map(|t| t.start).unwrap_or(total_duration);
            let window_start = (timed_line.start - align::WINDOW_PAD_SECS).max(prev_bound);
            let window_end = (timed_line.sing_end + align::WINDOW_PAD_SECS).min(next_bound);
            requests.push((orig_idx, words, window_start, window_end));
        }
        if requests.is_empty() {
            self.status =
                "No timed line has more than one word to align - nothing to do.".to_string();
            return;
        }

        let language = self.align_language;
        let vocal_model = self.align_vocal_model;
        let total = requests.len();

        let progress = Arc::new(AtomicU32::new(0));
        let result: Arc<Mutex<Option<AlignJobResult>>> = Arc::new(Mutex::new(None));
        let progress_clone = progress.clone();
        let result_clone = result.clone();

        self.status = format!(
            "Auto-aligning {total} line(s)… separating vocals, loading the {} alignment model \
             (downloading it the first time), and can take a while.",
            language.label()
        );
        self.align_job = Some(AlignJob { progress, result });

        std::thread::spawn(move || {
            // Isolate vocals first and align against that instead of the
            // full mix - the CTC speech model tracks singing far more
            // reliably without instrumentation underneath it, and the
            // separation model is already bundled with every release (see
            // vocals.rs), so this costs extra compute time but no new
            // download. Best-effort: if separation fails for any reason,
            // fall back to aligning against the original audio rather than
            // failing the whole run over what's meant to be an accuracy
            // improvement, not a hard requirement. Takes the first half of
            // the progress bar; alignment itself takes the second half.
            // The original audio the user was tapping timestamps against
            // (self.audio, used for playback) is never touched by any of
            // this - it only ever decodes its own separate copy here, on
            // this background thread, from whichever file path is chosen.
            let mut vocals_tmp: Option<PathBuf> = None;
            let align_source: PathBuf = {
                let progress_for_separation = progress_clone.clone();
                match vocals::separate_vocals_to_temp_wav(&audio_path, vocal_model, move |p| {
                    progress_for_separation.store((p * 500.0) as u32, Ordering::Relaxed);
                }) {
                    Ok(path) => {
                        vocals_tmp = Some(path.clone());
                        path
                    }
                    Err(_) => audio_path.clone(),
                }
            };
            progress_clone.store(500, Ordering::Relaxed);

            let mut aligner = match align::Aligner::load(&align_source, language) {
                Ok(a) => a,
                Err(e) => {
                    if let Some(p) = &vocals_tmp {
                        let _ = std::fs::remove_file(p);
                    }
                    *result_clone.lock().unwrap() = Some(Err(e.to_string()));
                    return;
                }
            };
            let mut outcomes = Vec::with_capacity(total);
            for (i, (line_idx, words, window_start, window_end)) in requests.into_iter().enumerate()
            {
                let word_refs: Vec<&str> = words.iter().map(|s| s.as_str()).collect();
                let outcome = aligner
                    .align_line(&word_refs, window_start, window_end)
                    .map_err(|e| e.to_string());
                outcomes.push(AlignOutcome {
                    line_idx,
                    result: outcome,
                });
                progress_clone.store(
                    500 + (((i + 1) as f32 / total as f32) * 500.0) as u32,
                    Ordering::Relaxed,
                );
            }
            if let Some(p) = &vocals_tmp {
                let _ = std::fs::remove_file(p);
            }
            *result_clone.lock().unwrap() = Some(Ok(outcomes));
        });
    }

    /// Checks on a running auto-align job, if any, applying successful
    /// per-line results to `self.lines` and summarizing into `self.status`
    /// once it finishes. Returns true while still in progress.
    fn poll_align_job(&mut self) -> bool {
        let Some(job) = &self.align_job else {
            return false;
        };
        let finished = job.result.lock().unwrap().take();
        let Some(outcome) = finished else {
            return true;
        };
        self.align_job = None;

        match outcome {
            Err(e) => {
                self.status = format!("Auto-align failed: {e}");
            }
            Ok(outcomes) => {
                let mut aligned_lines = 0usize;
                let mut aligned_words = 0usize;
                let mut failures: Vec<String> = Vec::new();
                for o in outcomes {
                    match o.result {
                        Ok(words) => {
                            if let Some(line) = self.lines.get_mut(o.line_idx) {
                                let word_times: Vec<(f64, f64)> =
                                    words.iter().map(|w| (w.start, w.end)).collect();
                                let unit_times =
                                    lyrics::spread_word_timing(&line.text, &word_times);
                                for (i, &(start, end)) in unit_times.iter().enumerate() {
                                    if let Some(slot) = line.word_overrides.get_mut(i) {
                                        *slot = Some(start);
                                    }
                                    if let Some(slot) = line.word_end_overrides.get_mut(i) {
                                        *slot = Some(end);
                                    }
                                }
                                // Keep the line's own start/sing_end in
                                // sync with the first/last aligned word,
                                // the same tracking behavior manual
                                // tapping/dragging have (see
                                // tap_word_start/tap_word_end and the
                                // timeline drag handler) - otherwise
                                // aligned word timing can land outside the
                                // line's own start/end, needing a second
                                // manual fix. Safe unconditionally: each
                                // word's timestamp already comes from
                                // align_line's own analysis window
                                // (`window_start`/`window_end` in
                                // start_word_alignment), which is itself
                                // already clamped to the neighboring
                                // lines' own bounds.
                                if let (Some(first), Some(last)) = (words.first(), words.last()) {
                                    line.start = Some(first.start);
                                    line.sing_end_override = Some(last.end);
                                }
                                aligned_lines += 1;
                                aligned_words += words.len();
                            }
                        }
                        Err(e) => {
                            let text = self
                                .lines
                                .get(o.line_idx)
                                .map(|l| l.text.clone())
                                .unwrap_or_default();
                            failures.push(format!("\"{text}\": {e}"));
                        }
                    }
                }
                let mut msg =
                    format!("Auto-aligned {aligned_words} word(s) across {aligned_lines} line(s).");
                if !failures.is_empty() {
                    let shown = failures
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("; ");
                    let more = failures.len().saturating_sub(3);
                    let more_note = if more > 0 {
                        format!(" (and {more} more)")
                    } else {
                        String::new()
                    };
                    msg.push_str(&format!(
                        " {} line(s) failed: {shown}{more_note}",
                        failures.len()
                    ));
                }
                self.status = msg;
            }
        }
        false
    }

    /// Re-parses the raw lyrics text box, carrying over as much existing
    /// timing as it safely can instead of wiping it wholesale (see
    /// [`lyrics::merge_reparsed_lyrics`]) - this is what makes it safe to
    /// fix a typo, or insert/reorder a line, without re-tapping the whole
    /// song from scratch.
    fn parse_lyrics(&mut self) {
        let edited_lines = self
            .lyrics_edited_span
            .take()
            .map(|span| char_span_to_line_range(&self.lyrics_raw, span));
        let (lines, report) =
            lyrics::merge_reparsed_lyrics(&self.lines, &self.lyrics_raw, edited_lines);
        self.lines = lines;
        // Some/all lines may already be timed after the merge above -
        // resume tapping at the first line that still needs it, rather
        // than always restarting from line 1.
        self.recompute_next_untimed();
        self.start_edit = None;
        self.end_edit = None;
        self.status = report.summarize(self.lines.len());
    }

    fn load_lyrics_file_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Lyrics files", &["lrc", "txt", "kok"])
            .add_filter("All files", &["*"])
            .pick_file()
        {
            self.load_lyrics_file(&path);
        }
    }

    /// Loads a lyrics file in any supported format (LRC, UltraStar, KOK, or
    /// plain text), auto-detected. Unlike the paste box, these formats
    /// often carry real timing (and sometimes word-level timing/duet voice
    /// assignment) already - imported lines land pre-timed where the file
    /// provides it, so tapping may be partly or entirely unnecessary.
    fn load_lyrics_file(&mut self, path: &std::path::Path) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                self.status = format!("Couldn't read {}: {e}", path.display());
                return;
            }
        };

        let format = formats::detect_format(&content);
        let result: Result<Vec<LyricLine>, String> = match format {
            formats::LyricFormat::Lrc => Ok(formats::import_lrc(&content)),
            formats::LyricFormat::UltraStar => formats::import_ultrastar(&content),
            formats::LyricFormat::Kok => Ok(formats::import_kok(&content)),
            formats::LyricFormat::PlainText => Ok(formats::import_plain_text(&content)),
        };

        match result {
            Ok(lines) if lines.is_empty() => {
                self.status = format!(
                    "Detected {} but found no lyric lines in the file.",
                    format.label()
                );
            }
            Ok(lines) => {
                let already_timed = lines.iter().filter(|l| l.start.is_some()).count();
                self.lyrics_raw = lines
                    .iter()
                    .map(|l| l.text.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                self.lyrics_edited_span = None;
                self.lines = lines;
                self.recompute_next_untimed();
                self.start_edit = None;
                self.end_edit = None;
                self.status = if already_timed > 0 {
                    format!(
                        "Loaded {} line(s) as {} - {} already timed. Play to fine-tune words, \
                         or export as-is. (Re-clicking \"Parse lyrics\" would reset this timing \
                         from the text box, so leave that alone unless you want to start over.)",
                        self.lines.len(),
                        format.label(),
                        already_timed
                    )
                } else {
                    format!(
                        "Loaded {} line(s) as {}. Play the song and tap along.",
                        self.lines.len(),
                        format.label()
                    )
                };
            }
            Err(e) => {
                self.status = format!(
                    "Couldn't parse {} as {}: {e}",
                    path.display(),
                    format.label()
                );
            }
        }
    }

    /// Routes files dropped onto the window to whichever loader matches
    /// their extension - audio, a lyrics file (any of the auto-detected
    /// formats `load_lyrics_file` handles), or a `.abyzl` project - so
    /// dragging a file in works as an alternative to every "Load…" button,
    /// not just one of them. Multiple files dropped at once (e.g. an audio
    /// file and a lyrics file together) are each routed independently.
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for file in dropped {
            let Some(path) = file.path else {
                let name = if file.name.is_empty() {
                    "dropped file".to_string()
                } else {
                    file.name.clone()
                };
                self.status = format!("Couldn't read \"{name}\" - no file path was given.");
                continue;
            };
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_default();
            match ext.as_str() {
                "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" => self.load_audio(path),
                "lrc" | "txt" | "kok" => self.load_lyrics_file(&path),
                project::FILE_EXTENSION => {
                    if self.project_busy() {
                        self.status =
                            "Can't load a project while an export is running.".to_string();
                    } else {
                        self.request_project_load(path);
                    }
                }
                _ => {
                    self.status = format!(
                        "Dropped file \"{}\" isn't a recognized audio, lyrics, or .{} project \
                         file.",
                        path.display(),
                        project::FILE_EXTENSION
                    );
                }
            }
        }
    }

    /// Full-window "drop it here" overlay shown while a file is being
    /// dragged over the window (before it's actually dropped) - otherwise
    /// drag-and-drop would be an invisible feature nothing on screen hints
    /// at.
    fn draw_drop_hint(&self, ctx: &egui::Context) {
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop_hint_overlay"),
        ));
        let screen = ctx.screen_rect();
        painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(180));
        painter.text(
            screen.center(),
            egui::Align2::CENTER_CENTER,
            format!(
                "Drop an audio, lyrics, or .{} project file",
                project::FILE_EXTENSION
            ),
            egui::FontId::proportional(24.0),
            egui::Color32::WHITE,
        );
    }

    /// Tap along: the first press for a line sets its start, the next
    /// press sets its end (see [`TapPhase`]), then advances to the next
    /// line's start - so both boundaries come from tapping in real time
    /// rather than only the start, with the end left to an estimate.
    fn tap_next(&mut self) {
        if let Some(pos) = self.live_tap_position() {
            self.tap_at(pos);
        }
    }

    /// The playback position to record a tap at, or `None` (with the
    /// reason in the status bar) if tapping isn't possible right now.
    fn live_tap_position(&mut self) -> Option<f64> {
        let Some(audio) = &self.audio else {
            return None;
        };
        if !audio.is_playing() {
            self.status = "Press Play first, then tap along with the song.".to_string();
            return None;
        }
        if self.next_untimed >= self.lines.len() {
            self.status = "All lines are already timed.".to_string();
            return None;
        }
        Some(compensate_tap(
            audio.position(),
            self.prefs.tap_latency_ms,
            audio.speed(),
        ))
    }

    /// Shift+Space: for lines sung back-to-back with no gap, one tap sets
    /// the current line's end *and* the next line's start at the same
    /// instant - two taps a split second apart are hard to land and leave a
    /// tiny false gap. Only meaningful while waiting for an end tap; at a
    /// start tap it's just a normal tap.
    fn tap_end_and_start_next(&mut self) {
        let Some(pos) = self.live_tap_position() else {
            return;
        };
        if self.tap_phase != TapPhase::End {
            self.tap_at(pos);
            return;
        }
        let ended = self.next_untimed;
        self.tap_at(pos);
        // Only chain on if the end tap actually landed and moved us on to
        // the next line's start (it stops on errors, or at the last line).
        if self.tap_phase == TapPhase::Start
            && self.next_untimed > ended
            && self.next_untimed < self.lines.len()
        {
            let end_status = self.status.clone();
            self.tap_at(pos);
            if self.tap_phase == TapPhase::End {
                self.status = format!(
                    "Line {} ended and line {} started at {} - tap again for its end.",
                    ended + 1,
                    self.next_untimed + 1,
                    lyrics::format_timecode(pos)
                );
            } else {
                self.status = format!("{end_status} {}", self.status);
            }
        }
    }

    /// Records one tap-along press at `pos` - see [`Self::tap_next`].
    fn tap_at(&mut self, pos: f64) {
        let idx = self.next_untimed;
        match self.tap_phase {
            TapPhase::Start => {
                let trimmed = match lyrics::apply_tap_start(&mut self.lines, idx, pos) {
                    Ok(t) => t,
                    Err(e) => {
                        self.status = e;
                        return;
                    }
                };
                self.tap_phase = TapPhase::End;
                self.status = match trimmed {
                    Some(p) => format!(
                        "Line {} started at {} (line {}'s end moved back to meet it) - tap \
                         again for its end.",
                        idx + 1,
                        lyrics::format_timecode(pos),
                        p + 1
                    ),
                    None => format!(
                        "Line {} started at {} - tap again for its end.",
                        idx + 1,
                        lyrics::format_timecode(pos)
                    ),
                };
            }
            TapPhase::End => {
                let (pos, clamped) = match lyrics::apply_tap_end(&mut self.lines, idx, pos) {
                    Ok(r) => r,
                    Err(e) => {
                        self.status = e;
                        return;
                    }
                };
                self.tap_phase = TapPhase::Start;
                // On to the next *main* line - backing lines are skipped
                // (see `tap_backing_line`).
                self.next_untimed = self.lines[idx + 1..]
                    .iter()
                    .position(|l| !l.is_backing)
                    .map(|off| idx + 1 + off)
                    .unwrap_or(self.lines.len());

                if self.next_untimed >= self.lines.len() && !self.lines.is_empty() {
                    // Just finished the last line - open the fine-tune-words
                    // panel automatically. It'll track along with playback
                    // from here, so the user can keep playing and just
                    // click words as they come up rather than having to
                    // manually select each line.
                    self.word_tap_line = Some(0);
                    self.status = "All lines timed! Click words as they're sung to fine-tune \
                                   them - it'll follow the song automatically. Space now \
                                   pauses/resumes playback instead of tapping."
                        .to_string();
                } else if clamped {
                    self.status = format!(
                        "Line {} ended at {} (where the next line already starts).",
                        idx + 1,
                        lyrics::format_timecode(pos)
                    );
                } else {
                    self.status = format!(
                        "Line {} ended at {}.",
                        idx + 1,
                        lyrics::format_timecode(pos)
                    );
                }
            }
        }
    }

    /// Space-bar behavior once there's nothing left to tap along (see the
    /// key handler in `update()`): play from the start if nothing's loaded
    /// yet, resume if paused, otherwise pause - the same as the transport's
    /// ▶ Play / ⏸ Pause toggle button, just combined into whichever one
    /// applies, so Space can drive playback hands-free while fine-tuning
    /// once every line is timed.
    fn toggle_play_pause(&mut self) {
        let is_playing = self.audio.as_ref().map(|a| a.is_playing()).unwrap_or(false);
        let is_paused = self.audio.as_ref().map(|a| a.is_paused()).unwrap_or(false);
        if let Some(a) = &mut self.audio {
            if is_playing {
                a.pause();
            } else if is_paused {
                a.resume();
            } else if let Err(e) = a.play_from_start() {
                self.status = format!("Playback error: {e}");
            }
        }
    }

    /// Finds the first line still missing a start or an end, and sets
    /// [`Self::next_untimed`]/[`Self::tap_phase`] to resume tapping there -
    /// used after any edit (manual entry, clearing a line, resetting) that
    /// could leave the tap-along workflow pointed somewhere stale.
    fn recompute_next_untimed(&mut self) {
        // Backing lines are timed separately (see `tap_backing_line`), so
        // tap-along never stops on one.
        self.next_untimed = self
            .lines
            .iter()
            .position(|l| !l.is_backing && (l.start.is_none() || l.sing_end_override.is_none()))
            .unwrap_or(self.lines.len());
        self.tap_phase = match self.lines.get(self.next_untimed) {
            Some(l) if l.start.is_some() => TapPhase::End,
            _ => TapPhase::Start,
        };
    }

    /// The line the M/F/D/S singer-assignment shortcuts apply to -
    /// whichever line is still waiting to be tapped, if tapping isn't done
    /// yet (so a voice can be set right before tapping its start without
    /// touching the mouse), otherwise whichever line the fine-tune-words
    /// panel is currently following.
    fn current_singer_assignment_target(&self) -> Option<usize> {
        if self.next_untimed < self.lines.len() {
            Some(self.next_untimed)
        } else {
            self.word_tap_line
        }
    }

    fn assign_singer_to_current_line(&mut self, singer: Singer) {
        let Some(idx) = self.current_singer_assignment_target() else {
            self.status = "No line to assign a singer to right now.".to_string();
            return;
        };
        if let Some(line) = self.lines.get_mut(idx) {
            line.singer = singer;
            self.status = format!("Line {} set to {}.", idx + 1, singer.label());
        }
    }

    /// While the fine-tune-words panel is open and the song is playing,
    /// keep it pointed at whichever line is currently active, so the user
    /// can just play through and click words without manually reselecting
    /// a line every time the song moves on to the next one. Does nothing
    /// when [`Self::auto_follow_words`] is turned off, so a line stays
    /// selected until the user moves on themselves - useful for a line's
    /// first/last word, which auto-follow can otherwise snatch the panel
    /// away from (or onto) right as it needs a click.
    fn auto_follow_word_tap_line(&mut self) {
        if !self.auto_follow_words || self.word_tap_line.is_none() {
            return;
        }
        // A line loop's lead-in plays the end of the previous line - don't
        // let that drag the panel off the line being looped.
        if matches!(self.loop_mode, Some(LoopMode::Line(_))) {
            return;
        }
        let Some(audio) = &self.audio else { return };
        if !audio.is_playing() {
            return;
        }
        let t = audio.position();
        let (timed, orig_indices) = self.resolved_with_indices();
        for (tl, &orig_idx) in timed.iter().zip(orig_indices.iter()) {
            if t >= tl.start && t < tl.end {
                self.word_tap_line = Some(orig_idx);
                break;
            }
        }
    }

    /// `(start, sing_end)` of line `idx` as currently resolved (explicit or
    /// estimated end), from the main or backing track - `None` if it isn't
    /// timed yet.
    fn line_bounds(&self, idx: usize) -> Option<(f64, f64)> {
        self.resolved_timed_line(idx).map(|t| (t.start, t.sing_end))
    }

    /// Line `idx` as currently resolved (main or backing track), or `None`
    /// if it isn't timed yet.
    fn resolved_timed_line(&self, idx: usize) -> Option<TimedLine> {
        let (timed, indices) = self.resolved_with_indices();
        if let Some(k) = indices.iter().position(|&i| i == idx) {
            return timed.into_iter().nth(k);
        }
        let (timed, indices) =
            lyrics::resolve_backing_track_indexed(&self.lines, &self.timing_settings);
        let k = indices.iter().position(|&i| i == idx)?;
        timed.into_iter().nth(k)
    }

    /// The `[from, to)` song-time range the active loop repeats, if it's
    /// complete (an A-B loop needs both points).
    fn loop_region(&self) -> Option<(f64, f64)> {
        let duration = self.audio.as_ref().and_then(|a| a.duration());
        let (from, to) = match self.loop_mode? {
            LoopMode::Line(i) => {
                let (s, e) = self.line_bounds(i)?;
                ((s - LOOP_PRE_ROLL_SECS).max(0.0), e + LOOP_POST_ROLL_SECS)
            }
            LoopMode::Ab { a, b } => (a, b?),
        };
        // Stop just short of the very end, since the playback position
        // never goes past the song's duration and couldn't cross `to`.
        let to = match duration {
            Some(d) => to.min(d - 0.05),
            None => to,
        };
        (to > from).then_some((from, to))
    }

    /// Called every frame: while playing, jumps back to the loop's start
    /// whenever playback crosses its end. Also drops a line loop whose
    /// line no longer exists (e.g. after re-parsing).
    fn apply_loop(&mut self) {
        if let Some(LoopMode::Line(i)) = self.loop_mode {
            if i >= self.lines.len() {
                self.loop_mode = None;
            }
        }
        let Some(audio) = &self.audio else { return };
        if !audio.is_playing() {
            self.loop_last_pos = None;
            return;
        }
        let pos = audio.position();
        let prev = self.loop_last_pos.replace(pos);
        let Some((from, to)) = self.loop_region() else {
            return;
        };
        if loop_should_wrap(prev, pos, to) {
            if let Some(a) = &mut self.audio {
                if let Err(e) = a.seek(from) {
                    self.status = format!("Loop seek failed: {e}");
                }
            }
            self.loop_last_pos = Some(from);
        }
    }

    /// Starts looping line `idx` (or, with `None`, the line open in the
    /// fine-tune panel, else the line being tapped) - or stops the loop if
    /// a line loop is already running. Opens the fine-tune panel on that
    /// line and starts playback from the loop's lead-in.
    fn toggle_loop_line(&mut self, idx: Option<usize>) {
        if let Some(LoopMode::Line(current)) = self.loop_mode {
            if idx.is_none() || idx == Some(current) {
                self.loop_mode = None;
                self.status = "Loop off.".to_string();
                return;
            }
        }
        let target = idx
            .or(self.word_tap_line)
            .or_else(|| (self.next_untimed < self.lines.len()).then_some(self.next_untimed));
        let Some(i) = target else {
            self.status = "Pick a line to loop first (click it in the table below).".to_string();
            return;
        };
        if self.line_bounds(i).is_none() {
            self.status = format!("Line {} isn't timed yet - tap its start first.", i + 1);
            return;
        }
        self.loop_mode = Some(LoopMode::Line(i));
        self.word_tap_line = Some(i);
        if let Some((from, _)) = self.loop_region() {
            self.seek_and_play(from);
        }
        self.status = format!(
            "Looping line {} - press L or Loop line again to stop.",
            i + 1
        );
    }

    /// A-B loop: "A" marks the current position as the loop start (and
    /// clears any old end point).
    fn set_loop_a(&mut self) {
        let Some(pos) = self.audio.as_ref().map(|a| a.position()) else {
            return;
        };
        self.loop_mode = Some(LoopMode::Ab { a: pos, b: None });
        self.status = format!(
            "Loop start (A) set at {} - press B at the end point.",
            lyrics::format_timecode(pos)
        );
    }

    /// A-B loop: "B" marks the current position as the loop end and starts
    /// looping (jumping back to A right away).
    fn set_loop_b(&mut self) {
        let Some(pos) = self.audio.as_ref().map(|a| a.position()) else {
            return;
        };
        let Some(LoopMode::Ab { a, .. }) = self.loop_mode else {
            self.status = "Set the loop start (A) first.".to_string();
            return;
        };
        if pos < a + MIN_AB_LOOP_SECS {
            self.status = "Loop end (B) must be after the start (A).".to_string();
            return;
        }
        self.loop_mode = Some(LoopMode::Ab { a, b: Some(pos) });
        self.seek_and_play(a);
        self.status = format!(
            "Looping {} - {}.",
            lyrics::format_timecode(a),
            lyrics::format_timecode(pos)
        );
    }

    /// Seeks to `pos` and makes sure playback is running from there.
    fn seek_and_play(&mut self, pos: f64) {
        let Some(a) = &mut self.audio else { return };
        let result = if a.is_playing() || a.is_paused() {
            a.seek(pos).map(|_| a.resume())
        } else {
            a.play_from_start().and_then(|_| a.seek(pos))
        };
        if let Err(e) = result {
            self.status = format!("Playback error: {e}");
        }
        self.loop_last_pos = None;
    }

    /// Same as [`Self::resolved`], but also returns each `TimedLine`'s index
    /// into `self.lines` (needed so the fine-tune panel can edit the right
    /// original line, since `resolved()` sorts by start time).
    fn resolved_with_indices(&self) -> (Vec<TimedLine>, Vec<usize>) {
        let duration_hint = self.audio.as_ref().and_then(|a| a.duration());
        lyrics::resolve_timing_indexed(&self.lines, duration_hint, &self.timing_settings)
    }

    /// Seeks playback to [`AUDITION_REWIND_SECS`] before `near_time`,
    /// preserving whether it's currently playing or paused (see
    /// `AudioPlayer::seek`) - so correcting a timestamp (typing a new
    /// value, nudging, or dragging a timeline bubble) immediately lands
    /// you just before it, ready to hear whether the correction landed
    /// right without manually scrubbing back yourself. No-op if no audio
    /// is loaded.
    fn audition_seek(&mut self, near_time: f64) {
        if let Some(audio) = &mut self.audio {
            let _ = audio.seek((near_time - AUDITION_REWIND_SECS).max(0.0));
        }
    }

    fn retap_line(&mut self, idx: usize) {
        let Some(audio) = &self.audio else { return };
        if !audio.is_playing() {
            self.status = "Press Play first, then tap.".to_string();
            return;
        }
        let pos = compensate_tap(audio.position(), self.prefs.tap_latency_ms, audio.speed());
        if let Err(e) = lyrics::check_start_change(&self.lines, idx, pos) {
            self.status = e;
            return;
        }
        self.lines[idx].start = Some(pos);
        self.recompute_next_untimed();
    }

    /// The per-row "Tap" button for a backing line (see
    /// [`lyrics::LyricLine::is_backing`]). Tap-along skips backing lines
    /// (their timing overlaps the main lines', so they can't fit into its
    /// one-line-at-a-time start/end sequence), so each backing line is
    /// tapped on its own instead: the first press sets its start, the next
    /// its end - and once both are set, the next press starts over with a
    /// new start. Usually done on a second play-through, after the main
    /// lines are tapped.
    fn tap_backing_line(&mut self, idx: usize) {
        let Some(audio) = &self.audio else { return };
        if !audio.is_playing() {
            self.status = "Press Play first, then tap.".to_string();
            return;
        }
        let pos = compensate_tap(audio.position(), self.prefs.tap_latency_ms, audio.speed());
        let Some(line) = self.lines.get(idx) else {
            return;
        };
        let wants_end = line.start.is_some_and(|s| s < pos) && line.sing_end_override.is_none();
        if wants_end {
            if let Err(e) = lyrics::check_end_change(&self.lines, idx, pos) {
                self.status = e;
                return;
            }
            self.lines[idx].sing_end_override = Some(pos);
            self.status = format!(
                "Backing line {} ended at {}.",
                idx + 1,
                lyrics::format_timecode(pos)
            );
        } else {
            self.lines[idx].sing_end_override = None;
            self.lines[idx].start = Some(pos);
            self.status = format!(
                "Backing line {} started at {} - tap it again for its end.",
                idx + 1,
                lyrics::format_timecode(pos)
            );
        }
    }

    /// Turns a line into a backing-track line, or back into a main one -
    /// see [`lyrics::LyricLine::is_backing`]. Switching to backing also
    /// gives a still-`Default` line the dedicated Backing voice color (and
    /// switching back undoes exactly that), so a backing line stands out
    /// without an extra dropdown trip. Tap-along is re-pointed afterwards,
    /// since it skips backing lines.
    fn toggle_backing_line(&mut self, idx: usize) {
        let Some(line) = self.lines.get_mut(idx) else {
            return;
        };
        line.is_backing = !line.is_backing;
        if line.is_backing && line.singer == Singer::Default && line.custom_singer_name.is_none() {
            line.singer = Singer::Backing;
        } else if !line.is_backing && line.singer == Singer::Backing {
            line.singer = Singer::Default;
        }
        self.status = if line.is_backing {
            format!(
                "Line {} is now a backing vocal - time it with its own Tap button; it can \
                 overlap any main line.",
                idx + 1
            )
        } else {
            format!("Line {} is back on the main track.", idx + 1)
        };
        self.recompute_next_untimed();
    }

    fn reset_timing(&mut self) {
        for l in &mut self.lines {
            l.start = None;
            l.sing_end_override = None;
            l.estimated_sing_duration = None;
        }
        self.recompute_next_untimed();
        self.status = "Timing cleared.".to_string();
    }

    /// Tapping the *first* word's start also moves the line's own `start`
    /// to match (checked against the same neighbor-overlap rule
    /// [`lyrics::check_start_change`] applies to a direct line-start
    /// edit) - keeping the line bubble and its first word bubble always in
    /// sync, instead of letting them drift apart and need a second manual
    /// fix. See the matching sync in the timeline drag handler and in
    /// `poll_align_job` for auto-align's own results.
    fn tap_word_start(&mut self, line_idx: usize, word_idx: usize) {
        let Some(audio) = &self.audio else { return };
        if !audio.is_playing() {
            self.status = "Press Play first, then click a word to set its time.".to_string();
            return;
        }
        let pos = compensate_tap(audio.position(), self.prefs.tap_latency_ms, audio.speed());
        if word_idx == 0 {
            if let Err(e) = lyrics::check_start_change(&self.lines, line_idx, pos) {
                self.status = e;
                return;
            }
        }
        if let Some(line) = self.lines.get_mut(line_idx) {
            if let Some(slot) = line.word_overrides.get_mut(word_idx) {
                *slot = Some(pos);
            }
            if word_idx == 0 {
                line.start = Some(pos);
            }
        }
    }

    /// Tap the moment a word's held-out highlight should stop advancing -
    /// lets a word's color-wipe finish (and freeze) *before* the next
    /// word's start, instead of always stretching to fill that whole gap.
    /// See [`lyrics::LyricLine::word_end_overrides`].
    ///
    /// Tapping the *last* word's end also moves the line's own
    /// `sing_end_override` to match, the same sync [`Self::tap_word_start`]
    /// does for the first word's start.
    fn tap_word_end(&mut self, line_idx: usize, word_idx: usize) {
        let Some(audio) = &self.audio else { return };
        if !audio.is_playing() {
            self.status = "Press Play first, then click a word to set its end time.".to_string();
            return;
        }
        let pos = compensate_tap(audio.position(), self.prefs.tap_latency_ms, audio.speed());
        let is_last_word = self
            .lines
            .get(line_idx)
            .is_some_and(|l| word_idx + 1 == l.words().len());
        if is_last_word {
            if let Err(e) = lyrics::check_end_change(&self.lines, line_idx, pos) {
                self.status = e;
                return;
            }
        }
        if let Some(line) = self.lines.get_mut(line_idx) {
            if let Some(slot) = line.word_end_overrides.get_mut(word_idx) {
                *slot = Some(pos);
            }
            if is_last_word {
                line.sing_end_override = Some(pos);
            }
        }
    }

    fn reset_word_overrides(&mut self, line_idx: usize) {
        if let Some(line) = self.lines.get_mut(line_idx) {
            for o in line.word_overrides.iter_mut() {
                *o = None;
            }
            for o in line.word_end_overrides.iter_mut() {
                *o = None;
            }
            line.sing_end_override = None;
        }
        self.status = "Word and end timing reset to automatic for this line.".to_string();
    }

    /// Tap the moment the line's *last* word actually finishes being sung.
    /// Without this, the last word's held-out color-wipe end is only ever
    /// the automatic word-count estimate ([`lyrics::estimate_sing_duration`]),
    /// which has no way to know when a manually-tapped last word was
    /// actually finished singing - so once every word's start is fine-tuned,
    /// this is what makes the *end* of the line's highlight accurate too.
    fn tap_line_end(&mut self, line_idx: usize) {
        let Some(audio) = &self.audio else { return };
        if !audio.is_playing() {
            self.status = "Press Play first, then tap when the line finishes.".to_string();
            return;
        }
        let pos = compensate_tap(audio.position(), self.prefs.tap_latency_ms, audio.speed());
        if let Err(e) = lyrics::check_end_change(&self.lines, line_idx, pos) {
            self.status = e;
            return;
        }
        if let Some(line) = self.lines.get_mut(line_idx) {
            line.sing_end_override = Some(pos);
        }
        self.recompute_next_untimed();
    }

    /// Opens the combined "Export…" dialog, pre-filling the base filename
    /// (if it's still empty) from the loaded audio's name or the song
    /// title, so there's usually nothing to type before exporting.
    fn open_export_dialog(&mut self) {
        if self.export_base_name.trim().is_empty() {
            let audio = self.audio.as_ref().and_then(|a| a.path());
            self.export_base_name = default_export_base_name(audio, &self.title);
        }
        self.show_export_dialog = true;
    }

    /// Whether any selected export uses lyric timing (as opposed to only
    /// audio exports like the instrumental/vocals stems).
    fn any_timed_export(&self) -> bool {
        self.export_cdg
            || self.export_video
            || self.export_lrc
            || self.export_ultrastar
            || self.export_ass
            || self.export_srt
    }

    /// Draws the combined export options window: which output(s) to
    /// produce, their shared output folder/base filename, and any
    /// per-output settings (video resolution/vocal removal, instrumental
    /// format) - replaces three separate export buttons and three separate
    /// save dialogs with one folder pick and one click.
    /// If exporting right now would silently drop content because the lyric
    /// timing isn't complete, returns why - `resolve_timing` (used by both
    /// the CDG encoder and the video renderer) simply skips any line
    /// without a start timestamp, so a partially-timed song would export
    /// "successfully" while quietly missing lines instead of failing
    /// loudly. Instrumental-only exports never touch lyric timing at all,
    /// so they're never blocked by this.
    fn missing_timing_reason(&self) -> Option<String> {
        if !self.any_timed_export() {
            return None;
        }
        missing_timing(&self.lines)
    }

    /// Draws the in-app documentation ("Codex") window, if open - a
    /// category/article sidebar plus a `CommonMarkViewer`-rendered main
    /// area. See `codex.rs`'s own module docs for what's in
    /// [`codex::all_articles`] and why.
    fn draw_codex(&mut self, ctx: &egui::Context) {
        if !self.codex_open {
            return;
        }
        let mut open = true;
        egui::Window::new("📖 Codex")
            .open(&mut open)
            .default_size([760.0, 560.0])
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(200.0);
                        egui::ScrollArea::vertical()
                            .id_source("codex_nav_scroll")
                            .show(ui, |ui| {
                                // Grouped by category, in the fixed order
                                // each category's first article appears in
                                // `codex::all_articles` - a real category
                                // picker (independent of article order)
                                // isn't worth it yet; every category today
                                // is still either one article, or (just
                                // "Learn the Code") a handful of chapters
                                // in their natural reading order.
                                let mut last_category = "";
                                for (i, article) in codex::all_articles().iter().enumerate() {
                                    if article.category != last_category {
                                        ui.add_space(if last_category.is_empty() {
                                            0.0
                                        } else {
                                            8.0
                                        });
                                        ui.label(
                                            egui::RichText::new(article.category).small().weak(),
                                        );
                                        last_category = article.category;
                                    }
                                    ui.selectable_value(&mut self.codex_selected, i, article.title);
                                }
                            });
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_source("codex_article_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if let Some(article) = codex::all_articles().get(self.codex_selected) {
                                egui_commonmark::CommonMarkViewer::new("codex_article").show(
                                    ui,
                                    &mut self.codex_cache,
                                    article.content,
                                );
                            }
                        });
                });
            });
        if !open {
            self.codex_open = false;
        }
    }

    /// Shows `DISCLAIMER.md` (see `codex.rs`, which also lists it as an
    /// article - same content, not a second copy) once on first launch,
    /// with only an acknowledge button and no close/X - the point is that
    /// it's actually read once, not that it can be dismissed by accident.
    /// Never shows again after that, on this machine, regardless of which
    /// project is open or gets loaded later - see `onboarding.rs`.
    fn draw_disclaimer_modal(&mut self, ctx: &egui::Context) {
        if !self.show_disclaimer_modal {
            return;
        }
        egui::Window::new("Before you start")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_max_width(520.0);
                egui::ScrollArea::vertical()
                    .id_source("disclaimer_modal_scroll")
                    .max_height(420.0)
                    .show(ui, |ui| {
                        egui_commonmark::CommonMarkViewer::new("disclaimer_modal").show(
                            ui,
                            &mut self.codex_cache,
                            codex::DISCLAIMER_TEXT,
                        );
                    });
                ui.add_space(8.0);
                ui.vertical_centered(|ui| {
                    if ui.button("I Understand").clicked() {
                        onboarding::acknowledge_disclaimer(APP_ID);
                        self.show_disclaimer_modal = false;
                    }
                });
            });
    }

    fn draw_export_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_export_dialog {
            return;
        }
        let mut open = true;
        egui::Window::new("Export…")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Choose what to export - all selected outputs share one folder and base filename:");
                ui.add_space(6.0);

                ui.checkbox(&mut self.export_cdg, "CD+Graphics (.cdg)");
                ui.add_enabled_ui(self.export_cdg, |ui| {
                    ui.indent("cdg_opts", |ui| {
                        ui.checkbox(
                            &mut self.export_cdg_zip,
                            "Also package as an MP3+G .zip (.cdg + .mp3)",
                        )
                        .on_hover_text(
                            "The format karaoke hosts and KJ software import songs in: one \
                             .zip holding the .cdg and an .mp3 with the same name. A song \
                             that isn't already an MP3 is converted (needs ffmpeg, same as \
                             video export). The loose .cdg and audio are still saved too.",
                        );
                        ui.add_enabled_ui(self.export_cdg_zip, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Zip audio:");
                                ui.radio_value(&mut self.zip_instrumental, false, "Original song");
                                ui.radio_value(
                                    &mut self.zip_instrumental,
                                    true,
                                    "Instrumental (vocals removed)",
                                )
                                .on_hover_text(
                                    "Most karaoke tracks are instrumentals. Runs vocal \
                                     removal (slow - see \"Removing vocals\"), shared with \
                                     any other vocal-removal export you've picked.",
                                );
                            });
                        });
                    });
                });

                ui.checkbox(&mut self.export_lrc, "Lyrics (.lrc)");
                ui.add_enabled_ui(self.export_lrc, |ui| {
                    ui.indent("lrc_opts", |ui| {
                        ui.checkbox(
                            &mut self.lrc_enhanced_words,
                            "Word-level (LRC2) - uncheck for plain line-level LRC1",
                        );
                    });
                });

                ui.checkbox(&mut self.export_ultrastar, "Lyrics (UltraStar .txt)");

                ui.checkbox(&mut self.export_ass, "Karaoke subtitles (.ass)")
                    .on_hover_text(
                        "Subtitles with a word-by-word color wipe, in your voice colors - \
                         plays in VLC/mpv, and loads into Aegisub, video editors (to burn \
                         lyrics onto your own footage) and most subtitle tools.",
                    );
                ui.checkbox(&mut self.export_srt, "Plain subtitles (.srt)")
                    .on_hover_text(
                        "One subtitle per lyric line, no word timing - the most widely \
                         supported format (YouTube captions, any player or editor).",
                    );

                ui.checkbox(&mut self.export_video, "Video (.mp4)");
                ui.add_enabled_ui(self.export_video, |ui| {
                    ui.indent("video_opts", |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Resolution:");
                            egui::ComboBox::from_id_source("export_video_res")
                                .selected_text(self.video_resolution.label())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.video_resolution,
                                        Resolution::Hd720,
                                        "720p (quick drafts)",
                                    );
                                    ui.selectable_value(
                                        &mut self.video_resolution,
                                        Resolution::Hd1080,
                                        "1080p",
                                    );
                                    ui.selectable_value(
                                        &mut self.video_resolution,
                                        Resolution::Uhd4k,
                                        "4K",
                                    );
                                });
                        });
                        ui.horizontal(|ui| {
                            ui.label("Shape:");
                            ui.radio_value(
                                &mut self.video_style.orientation,
                                video::Orientation::Landscape,
                                "16:9 (YouTube, TVs)",
                            );
                            ui.radio_value(
                                &mut self.video_style.orientation,
                                video::Orientation::Portrait,
                                "9:16 (Shorts, TikTok, Reels)",
                            );
                        });
                        let (fw, fh) = self.video_style.frame_size(self.video_resolution);
                        ui.label(egui::RichText::new(format!("{fw}x{fh}")).small().weak());
                        ui.horizontal(|ui| {
                            ui.label("Lyric size:");
                            ui.add(
                                egui::Slider::new(
                                    &mut self.video_style.lyric_size_pct,
                                    video::MIN_LYRIC_SIZE_PCT..=video::MAX_LYRIC_SIZE_PCT,
                                )
                                .suffix("%"),
                            )
                            .on_hover_text(
                                "Size of the lyric lines (and backing lane) - the title card \
                                 keeps its size. Long lines still shrink to fit the frame. \
                                 The live preview shows the change.",
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.video_style.outline, "Text outline")
                                .on_hover_text(
                                    "A dark outline around all text - keeps lyrics readable \
                                     over a busy image or video background.",
                                );
                            ui.checkbox(&mut self.video_style.shadow, "Drop shadow")
                                .on_hover_text("A soft dark shadow under all text.");
                        });
                        if self.video_style.outline || self.video_style.shadow {
                            ui.label(
                                egui::RichText::new(
                                    "Outline/shadow appear in the exported video only, not the \
                                     live preview.",
                                )
                                .small()
                                .weak(),
                            );
                        }
                        ui.horizontal(|ui| {
                            let mut fade = self.video_style.fade_out_secs > 0.0;
                            if ui
                                .checkbox(&mut fade, "Fade out audio at the end")
                                .changed()
                            {
                                self.video_style.fade_out_secs = if fade { 3.0 } else { 0.0 };
                            }
                            if fade {
                                ui.add(
                                    egui::DragValue::new(&mut self.video_style.fade_out_secs)
                                        .range(0.5..=15.0)
                                        .speed(0.1)
                                        .fixed_decimals(1)
                                        .suffix(" s"),
                                );
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("Codec:");
                            egui::ComboBox::from_id_source("export_video_codec")
                                .selected_text(self.video_codec.label())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.video_codec,
                                        VideoCodec::Av1,
                                        VideoCodec::Av1.label(),
                                    );
                                    ui.selectable_value(
                                        &mut self.video_codec,
                                        VideoCodec::H264,
                                        VideoCodec::H264.label(),
                                    );
                                });
                        });
                        if self.video_codec == VideoCodec::H264 {
                            help_text(ui, self.prefs.show_tips, "H.264 files are several times larger than AV1.", "H.264 exports are typically several times larger than AV1 \
                                     at the same quality - pick this only if you specifically \
                                     need compatibility with something that can't play AV1.");
                        }
                        ui.checkbox(&mut self.remove_vocals_for_video, "Remove vocals (slow)");
                        ui.checkbox(
                            &mut self.show_credit_line,
                            "Show \"Created with Abyssal CDG Creator\" credit under the title card",
                        );
                    });
                });

                ui.checkbox(&mut self.export_instrumental, "Instrumental audio");
                ui.checkbox(&mut self.export_vocals, "Vocals audio");
                ui.add_enabled_ui(self.export_instrumental || self.export_vocals, |ui| {
                    ui.indent("stem_opts", |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Format:");
                            egui::ComboBox::from_id_source("export_stem_format")
                                .selected_text(self.instrumental_format.label())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.instrumental_format,
                                        InstrumentalFormat::Mp3,
                                        "MP3",
                                    );
                                    ui.selectable_value(
                                        &mut self.instrumental_format,
                                        InstrumentalFormat::Wav,
                                        "WAV",
                                    );
                                });
                        });
                    });
                });

                if self.export_video || self.export_instrumental || self.export_vocals {
                    help_text(ui, self.prefs.show_tips, "Vocal removal is slow - it runs an AI model on your computer.", "Vocal removal runs a real ML separation model (UVR-MDX-NET) \
                             entirely in-app, on-device - nothing to install separately. It \
                             can still take a while, especially on a machine without a lot of \
                             CPU headroom.");
                }
                if self.export_ultrastar {
                    help_text(ui, self.prefs.show_tips, "UltraStar export has no real pitch data.", "UltraStar export has no pitch data (this app doesn't track \
                             melody) - every note is written at a constant placeholder pitch, \
                             so a game that scores pitch accuracy will treat the whole song as \
                             one fixed note. Lyrics and timing are real.");
                }
                if self.export_lrc || self.export_ultrastar {
                    if let Some(audio) = &self.audio {
                        if audio.is_loaded() {
                            help_text(ui, self.prefs.show_tips, "The song's audio is copied next to lyrics exports.", "The loaded audio is copied alongside lyrics exports too \
                                     (same base filename), same as the .cdg pairing.");
                        }
                    }
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.label("Base filename:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.export_base_name)
                            .desired_width(180.0),
                    );
                });
                ui.horizontal(|ui| {
                    if ui.button("Choose folder…").clicked() {
                        let mut dialog = rfd::FileDialog::new();
                        if let Some(dir) = &self.export_folder {
                            dialog = dialog.set_directory(dir);
                        }
                        if let Some(dir) = dialog.pick_folder() {
                            // Remembered across restarts (see `prefs.rs`).
                            self.prefs.export_folder = Some(dir.clone());
                            prefs::save(APP_ID, &self.prefs);
                            self.export_folder = Some(dir);
                        }
                    }
                    match &self.export_folder {
                        Some(dir) => ui.label(dir.display().to_string()),
                        None => ui.label(egui::RichText::new("(no folder chosen)").weak()),
                    };
                });

                let missing_timing = self.missing_timing_reason();
                if let Some(reason) = &missing_timing {
                    ui.add_space(6.0);
                    ui.colored_label(egui::Color32::from_rgb(230, 160, 40), format!("⚠ {reason}"));
                }
                self.draw_export_checklist(ui);

                ui.add_space(10.0);
                let can_export = (self.export_cdg
                    || self.export_lrc
                    || self.export_ass
                    || self.export_srt
                    || self.export_ultrastar
                    || self.export_video
                    || self.export_instrumental
                    || self.export_vocals)
                    && self.export_folder.is_some()
                    && !self.export_base_name.trim().is_empty()
                    && missing_timing.is_none();
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(can_export, |ui| {
                        if ui.button("Export").clicked() {
                            self.start_combined_export();
                        }
                    });
                    if ui.button("Cancel").clicked() {
                        self.show_export_dialog = false;
                    }
                });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(4.0);
                ui.strong("Batch export");
                help_text(ui, self.prefs.show_tips, "Export several saved projects at once, with these choices.", "Export several saved projects (.abyzl) with the choices above, into \
                         the same folder. Each song keeps its own colors, video settings, font \
                         and background, and is named after its audio file (or title). Songs \
                         that aren't fully timed - or are missing audio an output needs - are \
                         skipped and listed.");
                ui.horizontal(|ui| {
                    if ui.button("Choose projects…").clicked() {
                        if let Some(paths) = rfd::FileDialog::new()
                            .add_filter("Abyssal CDG project", &[project::FILE_EXTENSION])
                            .pick_files()
                        {
                            for p in paths {
                                if !self.batch_projects.contains(&p) {
                                    self.batch_projects.push(p);
                                }
                            }
                        }
                    }
                    if !self.batch_projects.is_empty() && ui.small_button("Clear").clicked() {
                        self.batch_projects.clear();
                    }
                    ui.label(format!("{} chosen", self.batch_projects.len()));
                });
                if !self.batch_projects.is_empty() {
                    let mut remove = None;
                    egui::ScrollArea::vertical()
                        .id_source("batch_projects_scroll")
                        .max_height(110.0)
                        .show(ui, |ui| {
                            for (i, p) in self.batch_projects.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    if ui.small_button("✖").on_hover_text("Remove").clicked() {
                                        remove = Some(i);
                                    }
                                    ui.label(
                                        p.file_name()
                                            .map(|n| n.to_string_lossy().to_string())
                                            .unwrap_or_default(),
                                    )
                                    .on_hover_text(p.display().to_string());
                                });
                            }
                        });
                    if let Some(i) = remove {
                        self.batch_projects.remove(i);
                    }
                }
                let can_batch = self.export_options().any()
                    && self.export_folder.is_some()
                    && !self.batch_projects.is_empty()
                    && self.combined_export.is_none();
                ui.add_enabled_ui(can_batch, |ui| {
                    if ui
                        .button(format!("Export {} project(s)", self.batch_projects.len()))
                        .clicked()
                    {
                        self.start_batch_export();
                    }
                });
            });
        if !open {
            self.show_export_dialog = false;
        }
    }

    fn start_combined_export(&mut self) {
        if self.combined_export.is_some() {
            self.status = "An export is already in progress.".to_string();
            return;
        }
        if let Some(reason) = self.missing_timing_reason() {
            self.status = reason;
            return;
        }
        let opts = self.export_options();
        if !opts.any() {
            self.status = "Choose at least one output to export.".to_string();
            return;
        }
        let Some(folder) = self.export_folder.clone() else {
            self.status = "Choose an output folder first.".to_string();
            return;
        };
        // Sanitized here, not just at the auto-fill above - `title` (one
        // of that auto-fill's own sources) comes straight from a `.abyzl`
        // project file with no validation, and a project file is
        // explicitly meant to be shared between users. Without this, a
        // crafted title like "/etc/cron.d/evil" or "../../../elsewhere"
        // would silently redirect every export output to an arbitrary
        // location instead of the folder actually picked below, the
        // moment someone opens a shared project and clicks Export without
        // noticing/editing the pre-filled name.
        let base = sanitize_export_base_name(self.export_base_name.trim());

        let inputs = self.export_inputs();
        if opts.needs_audio() && inputs.audio_path.is_none() {
            self.status =
                "Load an audio file first - video/instrumental/vocals/zip export need it."
                    .to_string();
            return;
        }

        let progress = Arc::new(AtomicU32::new(0));
        let result: Arc<Mutex<Option<Vec<ExportOutcome>>>> = Arc::new(Mutex::new(None));
        let progress_clone = progress.clone();
        let result_clone = result.clone();

        self.status = "Exporting…".to_string();
        self.show_export_dialog = false;
        self.combined_export = Some(CombinedExportHandle {
            progress,
            result,
            folder: folder.clone(),
        });

        std::thread::spawn(move || {
            let outcomes = run_export(&inputs, &opts, &folder, &base, &|p| {
                progress_clone.store((p * 1000.0) as u32, Ordering::Relaxed)
            });
            *result_clone.lock().unwrap() = Some(outcomes);
        });
    }

    /// Starts a batch export of [`Self::batch_projects`] with the export
    /// dialog's current choices, into the chosen export folder - see
    /// [`run_batch_export`]. Shares the single-export progress bar.
    fn start_batch_export(&mut self) {
        if self.combined_export.is_some() {
            self.status = "An export is already in progress.".to_string();
            return;
        }
        let opts = self.export_options();
        if !opts.any() {
            self.status = "Choose at least one output to export.".to_string();
            return;
        }
        let Some(folder) = self.export_folder.clone() else {
            self.status = "Choose an output folder first.".to_string();
            return;
        };
        if self.batch_projects.is_empty() {
            self.status = "Choose the project files to batch export first.".to_string();
            return;
        }
        let projects = self.batch_projects.clone();
        let progress = Arc::new(AtomicU32::new(0));
        let result: Arc<Mutex<Option<Vec<ExportOutcome>>>> = Arc::new(Mutex::new(None));
        let progress_clone = progress.clone();
        let result_clone = result.clone();
        self.status = format!("Batch exporting {} project(s)…", projects.len());
        self.batch_running = true;
        self.show_export_dialog = false;
        self.combined_export = Some(CombinedExportHandle {
            progress,
            result,
            folder: folder.clone(),
        });
        std::thread::spawn(move || {
            let outcomes = run_batch_export(&projects, &opts, &folder, &|p| {
                progress_clone.store((p * 1000.0) as u32, Ordering::Relaxed)
            });
            *result_clone.lock().unwrap() = Some(outcomes);
        });
    }

    /// Notices when the status message changes (any of the ~100 places
    /// that set it), classifies it, and starts a toast for anything that
    /// isn't routine Info.
    fn track_status(&mut self) {
        if self.status == self.status_seen {
            return;
        }
        self.status_seen = self.status.clone();
        self.status_kind = classify_status(&self.status);
        self.toast_until = self
            .status_kind
            .toast_secs()
            .filter(|_| !self.status.is_empty())
            .map(|secs| std::time::Instant::now() + std::time::Duration::from_secs_f32(secs));
    }

    /// "📂 Open folder" for the last export's folder, if there was one.
    fn open_folder_button(&mut self, ui: &mut egui::Ui) {
        if let Some(dir) = self.last_export_folder.clone() {
            if ui
                .small_button("📂 Open folder")
                .on_hover_text(dir.display().to_string())
                .clicked()
            {
                if let Err(e) = open_in_file_manager(&dir) {
                    self.status = format!("Couldn't open the folder: {e}");
                }
            }
        }
    }

    /// The toast in the bottom-right corner for the latest success/
    /// warning/error: its first line (an export's full list stays in the
    /// status line), fading out over its last half second. Click ✖ (or
    /// anywhere on it) to dismiss.
    fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some(until) = self.toast_until else {
            return;
        };
        let now = std::time::Instant::now();
        if now >= until {
            self.toast_until = None;
            return;
        }
        let remaining = (until - now).as_secs_f32();
        let opacity = (remaining / 0.5).min(1.0);
        let kind = self.status_kind;
        let first_line: String = self
            .status
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(140)
            .collect();
        let mut dismiss = false;
        egui::Area::new(egui::Id::new("status_toast"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_opacity(opacity);
                let color = kind.color(ui.visuals());
                let frame = egui::Frame::popup(ui.style())
                    .stroke(egui::Stroke::new(1.5_f32, color))
                    .inner_margin(egui::Margin::symmetric(12.0, 8.0));
                let resp = frame
                    .show(ui, |ui| {
                        ui.set_max_width(420.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(format!("{}{first_line}", kind.icon()))
                                    .color(color)
                                    .strong(),
                            );
                            self.open_folder_button(ui);
                            if ui.small_button("✖").on_hover_text("Dismiss").clicked() {
                                dismiss = true;
                            }
                        });
                    })
                    .response;
                if resp.interact(egui::Sense::click()).clicked() {
                    dismiss = true;
                }
            });
        if dismiss {
            self.toast_until = None;
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// "01:23.4 / 05:06.4" next to the seek bar. Click it to type a time
    /// (MM:SS.SS, or plain seconds) and press Enter to jump there; Escape
    /// or clicking away cancels.
    fn draw_time_readout(&mut self, ui: &mut egui::Ui, duration: Option<f64>) {
        let pos = self.audio.as_ref().map(|a| a.position()).unwrap_or(0.0);
        let total = format_time(duration.unwrap_or(0.0));
        match self.time_jump_edit.as_mut() {
            Some(text) => {
                let resp = ui.add(
                    egui::TextEdit::singleline(text)
                        .desired_width(70.0)
                        .hint_text("MM:SS"),
                );
                resp.request_focus();
                ui.label(format!("/ {total}"));
                if resp.lost_focus() {
                    let entered = text.trim().to_string();
                    let commit = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    self.time_jump_edit = None;
                    if commit && !entered.is_empty() {
                        match parse_jump_time(&entered) {
                            Some(t) => {
                                if let Some(a) = &mut self.audio {
                                    if let Err(e) = a.seek(t) {
                                        self.status = format!("Seek error: {e}");
                                    }
                                }
                            }
                            None => {
                                self.status =
                                    format!("Couldn't read \"{entered}\" as a time (try MM:SS.SS).")
                            }
                        }
                    }
                }
            }
            None => {
                if ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(format!("{} / {total}", format_time(pos)))
                                .monospace(),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_text("Click to type a time to jump to.")
                    .on_hover_cursor(egui::CursorIcon::Text)
                    .clicked()
                {
                    self.time_jump_edit = Some(format_time(pos));
                }
            }
        }
    }

    /// Opens the Copy timing dialog for the checked lines, pointed at the
    /// next place they repeat (see `lyrics::find_repeat`), else the lines
    /// right after them.
    fn open_copy_timing_dialog(&mut self) {
        let sources: Vec<usize> = self.selected_lines.iter().copied().collect();
        let Some(&last) = sources.last() else { return };
        let n = sources.len();
        let first_target = lyrics::find_repeat(&self.lines, &sources)
            .unwrap_or((last + 1).min(self.lines.len().saturating_sub(n)));
        self.copy_dialog = Some(CopyTimingDialog {
            sources,
            first_target,
        });
    }

    /// Runs the copy (see `lyrics::copy_timing`) with the repeat starting at
    /// `start`, reporting the outcome; closes the dialog on success.
    fn apply_copy_timing(&mut self, start: f64) {
        let Some(d) = &self.copy_dialog else { return };
        let (sources, first) = (d.sources.clone(), d.first_target);
        let n = sources.len();
        match lyrics::copy_timing(&mut self.lines, &sources, first, start) {
            Ok(differing) => {
                self.recompute_next_untimed();
                self.audition_seek(start);
                self.status = format!(
                    "Copied the timing of {n} line(s) onto lines {}-{}.{}",
                    first + 1,
                    first + n,
                    if differing > 0 {
                        format!(
                            " {differing} of them have different words, so only their line \
                             start/end were copied."
                        )
                    } else {
                        String::new()
                    }
                );
                self.copy_dialog = None;
            }
            Err(e) => self.status = e,
        }
    }

    /// The Copy timing dialog: which lines are copied, where to (with a
    /// line-by-line preview of whether the words match), and when the
    /// repeat starts - its existing start, or tapped live while playing.
    fn draw_copy_timing_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &self.copy_dialog else { return };
        let sources = d.sources.clone();
        let mut first_target = d.first_target;
        let n = sources.len();
        let len = self.lines.len();
        let mut open = true;
        let mut apply_at: Option<f64> = None;
        let mut cancel = false;
        egui::Window::new("📋 Copy timing")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                let src_start = sources.first().and_then(|&i| self.lines[i].start);
                ui.label(format!(
                    "Copy the timing of {n} line(s), starting at line {} ({}), onto the repeat \
                     starting at line:",
                    sources[0] + 1,
                    src_start.map(lyrics::format_timecode).unwrap_or_default()
                ));
                ui.horizontal(|ui| {
                    let mut shown = first_target + 1;
                    ui.add(egui::DragValue::new(&mut shown).range(1..=len.saturating_sub(n) + 1));
                    first_target = shown.saturating_sub(1).min(len.saturating_sub(n));
                    if ui
                        .small_button("Find repeat")
                        .on_hover_text("Jump to the next lines with the same words.")
                        .clicked()
                    {
                        if let Some(j) = lyrics::find_repeat(&self.lines, &sources) {
                            first_target = j;
                        }
                    }
                });
                ui.add_space(4.0);
                egui::Grid::new("copy_timing_preview")
                    .num_columns(3)
                    .spacing([10.0, 2.0])
                    .show(ui, |ui| {
                        for (k, &src) in sources.iter().enumerate() {
                            let Some(dst) = self.lines.get(first_target + k) else {
                                break;
                            };
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}. {}",
                                    src + 1,
                                    lyrics::normalize_text(&self.lines[src].text)
                                ))
                                .weak(),
                            );
                            ui.label(format!(
                                "→ {}. {}",
                                first_target + k + 1,
                                lyrics::normalize_text(&dst.text)
                            ));
                            let same_units = lyrics::timing_units(&dst.text).len()
                                == lyrics::timing_units(&self.lines[src].text).len();
                            if same_units {
                                ui.colored_label(STEP_DONE, "✔").on_hover_text(
                                    "Same number of words - every word's timing is copied.",
                                );
                            } else {
                                ui.colored_label(egui::Color32::from_rgb(230, 160, 40), "≠")
                                    .on_hover_text(
                                        "Different words - only the line's start/end is \
                                         copied; its words go back to the estimate.",
                                    );
                            }
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
                ui.label("The repeat starts:");
                let target_start = self.lines.get(first_target).and_then(|l| l.start);
                ui.horizontal_wrapped(|ui| {
                    if let Some(t) = target_start {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(format!(
                                        "Where line {} already starts ({})",
                                        first_target + 1,
                                        lyrics::format_timecode(t)
                                    ))
                                    .color(egui::Color32::WHITE),
                                )
                                .fill(ACCENT),
                            )
                            .clicked()
                        {
                            apply_at = Some(t);
                        }
                        ui.label("or");
                    }
                    let playing = self.audio.as_ref().is_some_and(|a| a.is_playing());
                    ui.add_enabled_ui(playing, |ui| {
                        if ui
                            .button("⏱ It starts now")
                            .on_hover_text(
                                "While the song plays, click the instant the repeat's first \
                                 line starts being sung.",
                            )
                            .clicked()
                        {
                            if let Some(a) = &self.audio {
                                apply_at = Some(compensate_tap(
                                    a.position(),
                                    self.prefs.tap_latency_ms,
                                    a.speed(),
                                ));
                            }
                        }
                    });
                    if !playing {
                        ui.label(
                            egui::RichText::new("(play the song to tap it)")
                                .small()
                                .weak(),
                        );
                    }
                });
                ui.add_space(4.0);
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        if let Some(d) = &mut self.copy_dialog {
            d.first_target = first_target;
        }
        if !open || cancel {
            self.copy_dialog = None;
        } else if let Some(t) = apply_at {
            self.apply_copy_timing(t);
        }
    }

    /// Applies the saved UI size and the app's accent color on the first
    /// frame; afterwards, saves the UI size whenever it changes (the top
    /// panel's buttons, or egui's own Ctrl+= / Ctrl+- / Ctrl+0).
    fn sync_ui_style(&mut self, ctx: &egui::Context) {
        // A zoom change only takes effect at the start of a later frame, so
        // the first few frames after applying the saved size are left alone -
        // otherwise a not-yet-applied size could be "noticed" as a change
        // and saved over the user's setting.
        const SETTLE_FRAMES: u8 = 3;
        let Some(frames) = self.ui_style_frames else {
            self.ui_style_frames = Some(0);
            ctx.set_zoom_factor(self.prefs.ui_zoom);
            ctx.style_mut(|style| {
                // Selected items, slider/progress fills and text selection
                // in the accent purple - the same color as the primary
                // buttons and the step bar's current step.
                style.visuals.selection.bg_fill = ACCENT;
                style.visuals.selection.stroke.color = egui::Color32::WHITE;
                style.visuals.hyperlink_color = egui::Color32::from_rgb(185, 150, 245);
            });
            return;
        };
        if frames < SETTLE_FRAMES {
            self.ui_style_frames = Some(frames + 1);
            return;
        }
        let zoom = ctx.zoom_factor();
        if (zoom - self.prefs.ui_zoom).abs() > 0.001 {
            let clamped = zoom.clamp(prefs::MIN_UI_ZOOM, prefs::MAX_UI_ZOOM);
            if (clamped - zoom).abs() > 0.001 {
                ctx.set_zoom_factor(clamped);
            }
            self.prefs.ui_zoom = clamped;
            prefs::save(APP_ID, &self.prefs);
        }
    }

    fn workflow_snapshot(&self) -> WorkflowSnapshot {
        WorkflowSnapshot {
            has_audio: self.audio.as_ref().is_some_and(|a| a.is_loaded()),
            has_lyrics_text: !self.lyrics_raw.trim().is_empty(),
            lines: self.lines.len(),
            timed: self.lines.iter().filter(|l| l.start.is_some()).count(),
            estimated: checks::lines_on_estimated_timing(&self.lines).len(),
            exported: self.exported_this_project,
        }
    }

    /// The step bar across the top: Audio → Lyrics → Tap → Fine-tune →
    /// Export, each ticked off as it's done, then one highlighted "Next"
    /// line saying what to do now, with a button that does it.
    fn draw_step_bar(&mut self, ui: &mut egui::Ui) {
        let snap = self.workflow_snapshot();
        let (statuses, next) = workflow_status(&snap);
        ui.horizontal_wrapped(|ui| {
            for (k, (step, status)) in WorkflowStep::ALL.iter().zip(statuses).enumerate() {
                if k > 0 {
                    ui.label(egui::RichText::new("›").weak());
                }
                let (fill, text_color, mark) = match status {
                    StepStatus::Done => (STEP_DONE, egui::Color32::WHITE, "✔".to_string()),
                    StepStatus::Current => (ACCENT, egui::Color32::WHITE, format!("{}", k + 1)),
                    StepStatus::Optional => (
                        ui.visuals().faint_bg_color,
                        ui.visuals().weak_text_color(),
                        "◇".to_string(),
                    ),
                    StepStatus::Todo => (
                        ui.visuals().faint_bg_color,
                        ui.visuals().weak_text_color(),
                        format!("{}", k + 1),
                    ),
                };
                let mut label = format!("{mark}  {}", step.label());
                if *step == WorkflowStep::Tap && snap.lines > 0 && status != StepStatus::Done {
                    label.push_str(&format!("  {}/{}", snap.timed, snap.lines));
                }
                egui::Frame::none()
                    .fill(fill)
                    .rounding(10.0)
                    .inner_margin(egui::Margin::symmetric(10.0, 3.0))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(label).color(text_color).strong());
                    })
                    .response
                    .on_hover_text(step.hint());
            }
        });

        // The "Next" line.
        let all_done = statuses.iter().all(|s| *s == StepStatus::Done);
        let message = match next {
            WorkflowStep::Audio => {
                "Next: load the song's audio file (or drop it onto the window).".to_string()
            }
            WorkflowStep::Lyrics if snap.has_lyrics_text => {
                "Next: click Parse lyrics to turn the pasted text into lines.".to_string()
            }
            WorkflowStep::Lyrics => "Next: paste the lyrics into box 1 on the left (one line \
                                     per line of text), or load a lyrics file."
                .to_string(),
            WorkflowStep::Tap => format!(
                "Next: press Play and tap Space the instant each line starts, and again when \
                 it ends - {} of {} line(s) timed so far.",
                snap.timed, snap.lines
            ),
            _ if all_done => "Exported! Keep tweaking and export again any time.".to_string(),
            _ if snap.estimated > 0 => format!(
                "Every line is timed - export now, or first fine-tune the word timing of {} \
                 line(s) (click words as they're sung, or Auto-align).",
                snap.estimated
            ),
            _ => "Everything's timed - ready to export.".to_string(),
        };
        egui::Frame::none()
            .stroke(egui::Stroke::new(1.0_f32, ACCENT))
            .rounding(6.0)
            .inner_margin(egui::Margin::symmetric(8.0, 4.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(message).strong());
                    let primary = |text: &str| {
                        egui::Button::new(egui::RichText::new(text).color(egui::Color32::WHITE))
                            .fill(ACCENT)
                    };
                    match next {
                        WorkflowStep::Audio => {
                            if ui.add(primary("🎵 Load audio…")).clicked() {
                                self.load_audio_dialog();
                            }
                        }
                        WorkflowStep::Lyrics if snap.has_lyrics_text => {
                            if ui.add(primary("Parse lyrics")).clicked() {
                                self.parse_lyrics();
                            }
                        }
                        WorkflowStep::Lyrics => {
                            if ui.add(primary("Load lyrics file…")).clicked() {
                                self.load_lyrics_file_dialog();
                            }
                        }
                        WorkflowStep::Tap => {
                            let playing = self.audio.as_ref().is_some_and(|a| a.is_playing());
                            if !playing && ui.add(primary("▶ Start tapping")).clicked() {
                                self.start_tapping_from_next_line();
                            }
                        }
                        _ => {
                            if ui.add(primary("Export…")).clicked() {
                                self.open_export_dialog();
                            }
                            if snap.estimated > 0
                                && self.align_job.is_none()
                                && ui
                                    .button("🪄 Auto-align words")
                                    .on_hover_text(
                                        "Fill in every timed line's word timing from the audio.",
                                    )
                                    .clicked()
                            {
                                self.start_word_alignment();
                            }
                        }
                    }
                });
            });
    }

    /// The step bar's "Start tapping": plays from a couple of seconds
    /// before the last tapped point (the start of the song if nothing is
    /// tapped yet), so picking up where you left off is one click.
    fn start_tapping_from_next_line(&mut self) {
        let resume = self.lines[..self.next_untimed.min(self.lines.len())]
            .iter()
            .rev()
            .filter(|l| !l.is_backing)
            .find_map(|l| l.sing_end_override.or(l.start))
            .map(|t| (t - 2.0).max(0.0))
            .unwrap_or(0.0);
        self.seek_and_play(resume);
        self.status = "Tap Space the instant each line starts, and again when it ends.".to_string();
    }

    /// The export dialog's current "what to export, and how" choices.
    fn export_options(&self) -> ExportOptions {
        ExportOptions {
            cdg: self.export_cdg,
            lrc: self.export_lrc,
            ultrastar: self.export_ultrastar,
            ass: self.export_ass,
            srt: self.export_srt,
            video: self.export_video,
            instrumental: self.export_instrumental,
            vocals: self.export_vocals,
            zip: self.export_cdg_zip,
            zip_instrumental: self.zip_instrumental,
            lrc_enhanced: self.lrc_enhanced_words,
            remove_vocals_for_video: self.remove_vocals_for_video,
            instrumental_ext: self.instrumental_format.extension(),
        }
    }

    /// Everything about the open song an export needs (see [`ExportInputs`]).
    fn export_inputs(&self) -> ExportInputs {
        ExportInputs {
            lines: self.lines.clone(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            colors: self.export_colors(),
            timing_settings: self.timing_settings,
            video_resolution: self.video_resolution,
            video_codec: self.video_codec,
            video_style: self.video_style,
            outro: self.outro.clone(),
            show_credit_line: self.show_credit_line,
            background: self.background.clone(),
            background_fit: self.background_fit,
            background_dim: self.background_dim,
            font_family: self.selected_font_family.clone(),
            font_bytes: self.loaded_font_bytes(),
            audio_path: self
                .audio
                .as_ref()
                .and_then(|a| a.path())
                .map(Path::to_path_buf),
            audio_duration: self.audio.as_ref().and_then(|a| a.duration()),
        }
    }

    /// Checks on a running combined export, if any, updating status when
    /// it finishes. Returns true while still in progress (so the caller
    /// knows to keep repainting for the progress bar).
    fn poll_combined_export(&mut self) -> bool {
        let Some(handle) = &self.combined_export else {
            return false;
        };
        let finished = handle.result.lock().unwrap().take();
        if let Some(outcomes) = finished {
            let any_err = outcomes.iter().any(|o| o.result.is_err());
            let any_ok = outcomes.iter().any(|o| o.result.is_ok());
            if any_ok && !self.batch_running {
                self.exported_this_project = true;
            }
            if any_ok {
                self.last_export_folder = Some(handle.folder.clone());
            }
            self.batch_running = false;
            let lines: Vec<String> = outcomes
                .iter()
                .map(|o| match &o.result {
                    Ok(path) => format!("{}: saved to {}", o.label, path.display()),
                    Err(e) => format!("{}: failed - {e}", o.label),
                })
                .collect();
            let heading = if any_err && any_ok {
                "Export finished with some errors:"
            } else if any_err {
                "Export failed:"
            } else {
                "Export complete!"
            };
            self.status = format!("{heading}\n{}", lines.join("\n"));
            self.combined_export = None;
            false
        } else {
            true
        }
    }

    /// Shows the popped-out preview as its own native window (an egui
    /// "immediate" viewport - drawn as part of the main window's own frame,
    /// so it repaints in step with it during playback with no extra
    /// syncing). Letterboxed to 16:9 on black, so it can be resized or
    /// maximized to any shape. Closing the window docks the preview back
    /// into the right panel.
    /// The export dialog's pre-export checklist (see `checks.rs`): a
    /// collapsible list of warnings/notes for whatever's about to be
    /// exported, each with a "Show" button (jumps to the line) and, where
    /// there's an obvious fix, a button for that. Never blocks exporting.
    fn draw_export_checklist(&mut self, ui: &mut egui::Ui) {
        let targets = checks::Targets {
            cdg: self.export_cdg,
            timed: self.any_timed_export(),
        };
        if !targets.cdg && !targets.timed {
            return;
        }
        let duration = self.audio.as_ref().and_then(|a| a.duration());
        let mut issues = checks::run(
            &self.lines,
            &self.title,
            &self.artist,
            &self.timing_settings,
            duration,
            targets,
        );
        // Color readability (see the Colors panel) - only for outputs that
        // show colors.
        if self.export_cdg || self.export_video || self.export_ass {
            for warning in color_warnings(&self.export_colors(), &self.voices_in_use()) {
                issues.push(checks::Issue {
                    severity: checks::Severity::Note,
                    message: format!("Colors: {warning}"),
                    line: None,
                    fix: None,
                });
            }
        }
        let warnings = issues
            .iter()
            .filter(|i| i.severity == checks::Severity::Warning)
            .count();
        let notes = issues.len() - warnings;
        let amber = egui::Color32::from_rgb(230, 160, 40);
        ui.add_space(6.0);
        if issues.is_empty() {
            ui.label("✅ Checklist: nothing to fix.");
            return;
        }
        let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
        let header = match (warnings, notes) {
            (0, n) => format!("ℹ Checklist: {}", plural(n, "note")),
            (w, 0) => format!("⚠ Checklist: {}", plural(w, "warning")),
            (w, n) => format!(
                "⚠ Checklist: {}, {}",
                plural(w, "warning"),
                plural(n, "note")
            ),
        };
        let mut show_line = None;
        let mut fix = None;
        egui::CollapsingHeader::new(egui::RichText::new(header).color(if warnings > 0 {
            amber
        } else {
            ui.visuals().text_color()
        }))
        .id_source("export_checklist")
        .default_open(warnings > 0)
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(
                    "Worth a look before exporting - none of these stop the export.",
                )
                .small()
                .weak(),
            );
            egui::ScrollArea::vertical()
                .id_source("export_checklist_scroll")
                .max_height(220.0)
                .show(ui, |ui| {
                    for issue in &issues {
                        ui.horizontal_wrapped(|ui| {
                            match issue.severity {
                                checks::Severity::Warning => ui.colored_label(amber, "⚠"),
                                checks::Severity::Note => ui.label("ℹ"),
                            };
                            ui.label(&issue.message);
                            if let Some(i) = issue.line {
                                if ui.small_button("Show").clicked() {
                                    show_line = Some(i);
                                }
                            }
                            if let Some(checks::Fix::SplitLine(i)) = issue.fix {
                                if ui
                                    .small_button("Split line")
                                    .on_hover_text(
                                        "Split it into two lines at the middle word, keeping \
                                         its timing - updates the lyrics box too. Ctrl+Z \
                                         undoes it.",
                                    )
                                    .clicked()
                                {
                                    fix = Some(checks::Fix::SplitLine(i));
                                }
                            }
                        });
                        ui.add_space(2.0);
                    }
                });
        });
        if let Some(i) = show_line {
            self.show_line(i);
        }
        if let Some(checks::Fix::SplitLine(i)) = fix {
            self.split_line_in_two(i);
        }
    }

    /// Brings line `i` into view: opens it in the fine-tune panel, scrolls
    /// the timing table to it, and moves playback to just before it.
    fn show_line(&mut self, i: usize) {
        if i >= self.lines.len() {
            return;
        }
        self.word_tap_line = Some(i);
        self.timing_table_scroll_request = Some(i);
        if let Some((start, _)) = self.line_bounds(i) {
            if let Some(a) = &mut self.audio {
                let _ = a.seek((start - 1.0).max(0.0));
            }
        }
        self.status = format!("Line {}: {}", i + 1, self.lines[i].text);
    }

    /// The checklist's "Split line" fix - see `lyrics::split_line`.
    fn split_line_in_two(&mut self, i: usize) {
        let Some(line) = self.lines.get(i) else {
            return;
        };
        let Some(at_word) = lyrics::best_split_word(&line.text) else {
            return;
        };
        // When the second half's first word is highlighted - manual or
        // estimated - becomes the split point in time.
        let split_time = self.resolved_timed_line(i).and_then(|t| {
            lyrics::word_timings(&t)
                .get(at_word)
                .map(|w| w.highlight_at)
        });
        match lyrics::split_line(
            &mut self.lines,
            &mut self.lyrics_raw,
            i,
            at_word,
            split_time,
        ) {
            Ok(()) => {
                self.lyrics_edited_span = None;
                self.selected_lines.clear();
                if self.word_tap_line.is_some_and(|w| w > i) {
                    self.word_tap_line = self.word_tap_line.map(|w| w + 1);
                }
                self.recompute_next_untimed();
                self.status = format!("Split line {} into lines {} and {}.", i + 1, i + 1, i + 2);
            }
            Err(e) => self.status = e,
        }
    }

    fn show_preview_window(&mut self, ctx: &egui::Context) {
        let builder = egui::ViewportBuilder::default()
            .with_title("Abyssal CDG Creator - Preview")
            .with_inner_size([960.0, 540.0])
            .with_min_inner_size([320.0, 180.0]);
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("preview_window"),
            builder,
            |ctx, class| {
                if class == egui::ViewportClass::Embedded {
                    // Backend without multi-window support - fall back to
                    // a floating window inside the main one.
                    let mut open = true;
                    egui::Window::new("Preview")
                        .open(&mut open)
                        .resizable(true)
                        .default_size([640.0, 360.0])
                        .show(ctx, |ui| {
                            let h = ui.available_height();
                            self.draw_preview(ui, Some(h));
                        });
                    if !open {
                        self.preview_popped_out = false;
                    }
                    return;
                }

                self.handle_keyboard_shortcuts(ctx);
                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(egui::Color32::BLACK))
                    .show(ctx, |ui| {
                        let avail = ui.available_size();
                        let aspect = match self.video_style.orientation {
                            video::Orientation::Landscape => 16.0 / 9.0,
                            video::Orientation::Portrait => 9.0 / 16.0,
                        };
                        let height = avail.x.min(avail.y * aspect) / aspect;
                        ui.add_space(((avail.y - height) / 2.0).max(0.0));
                        ui.vertical_centered(|ui| self.draw_preview(ui, Some(avail.y)));
                    });
                if ctx.input(|i| i.viewport().close_requested()) {
                    self.preview_popped_out = false;
                }
            },
        );
    }

    /// The app's keyboard shortcuts (save, tap/play, seek, undo/redo,
    /// singer keys). Called for the main window and, while it's open, for
    /// the popped-out preview window too - `ctx.input` only ever sees the
    /// window currently being drawn's own key presses, so a press is
    /// handled exactly once, by whichever window had focus. That keeps
    /// tapping along with Space working while you watch the big preview.
    fn handle_keyboard_shortcuts(&mut self, ctx: &egui::Context) {
        // Ctrl+S / Ctrl+Shift+S save the project - unlike the shortcuts
        // below, these are safe to fire even while a text field has focus,
        // the same way most apps let Ctrl+S through while you're typing.
        if ctx.input(|i| i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::S)) {
            self.save_project();
        }
        if ctx.input(|i| i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::S)) {
            self.save_project_as_dialog();
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::E)) {
            self.open_export_dialog();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.show_shortcuts = !self.show_shortcuts;
        }

        // Keyboard shortcuts - only when no text field etc. has focus, so
        // typing a space in the lyrics box or title/artist fields doesn't
        // accidentally trigger a tap.
        let typing = ctx.memory(|m| m.focused().is_some());
        if !typing {
            if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
                // Once there's nothing left to tap (no lyrics parsed yet,
                // or every line already has both a start and an end),
                // Space drives playback instead - much less hunting for the
                // mouse while fine-tuning with the timeline/word panel.
                if self.next_untimed >= self.lines.len() {
                    self.toggle_play_pause();
                } else if ctx.input(|i| i.modifiers.shift) {
                    self.tap_end_and_start_next();
                } else {
                    self.tap_next();
                }
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
                if let Some(a) = &mut self.audio {
                    let target = (a.position() - 2.0).max(0.0);
                    let _ = a.seek(target);
                }
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
                if let Some(a) = &mut self.audio {
                    let target = a.position() + 2.0;
                    let _ = a.seek(target);
                }
            }
            // Undo/redo - gated the same way as the shortcuts above (not
            // while a text field has focus, so a field's own native
            // undo/redo isn't stolen by this), and additionally not
            // mid-drag on the timeline, where applying a snapshot from
            // before the drag started would fight with the drag still
            // updating state every frame.
            if self.timeline_drag.is_none() {
                if ctx.input(|i| {
                    i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::Z)
                }) {
                    self.undo();
                }
                if ctx.input(|i| {
                    (i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::Z))
                        || (i.modifiers.command && i.key_pressed(egui::Key::Y))
                }) {
                    self.redo();
                }
            }
            // Singer assignment - M/F/D/S set the voice of whichever line
            // is next up to tap (mid-tapping) or, once everything's timed,
            // whichever line the fine-tune-words panel is following - so a
            // duet/multi-voice song's singer switches can be set with the
            // same "hands stay on the keyboard while playing" flow as
            // tapping itself, without reaching for each line's dropdown.
            // Plain presses only (`!modifiers.command`), so this can't
            // fire as a side effect of e.g. Ctrl+S.
            if ctx.input(|i| i.key_pressed(egui::Key::M) && !i.modifiers.command) {
                self.assign_singer_to_current_line(Singer::Male);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::F) && !i.modifiers.command) {
                self.assign_singer_to_current_line(Singer::Female);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::D) && !i.modifiers.command) {
                self.assign_singer_to_current_line(Singer::Duet);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::S) && !i.modifiers.command) {
                self.assign_singer_to_current_line(Singer::Screaming);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::L) && !i.modifiers.command) {
                self.toggle_loop_line(None);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::B) && !i.modifiers.command) {
                self.assign_singer_to_current_line(Singer::Backing);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Questionmark)) {
                self.show_shortcuts = !self.show_shortcuts;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Home)) {
                if let Some(a) = &mut self.audio {
                    let _ = a.seek(0.0);
                }
            }
            // , / . nudge the current line's start; with Shift, shift the
            // whole selection (start, end and word timing together).
            for (key, delta) in [(egui::Key::Comma, -0.1), (egui::Key::Period, 0.1)] {
                let (pressed, shift) = ctx.input(|i| {
                    (
                        i.key_pressed(key) && !i.modifiers.command,
                        i.modifiers.shift,
                    )
                });
                if !pressed {
                    continue;
                }
                if shift {
                    self.shift_selection(delta);
                } else if let Some(i) = self.edit_target() {
                    self.nudge_line_start(i, delta);
                } else {
                    self.status = "Tap a line (or click one in the table) to nudge it.".to_string();
                }
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.command) {
                match self.edit_target() {
                    Some(i) if self.lines[i].is_backing => self.tap_backing_line(i),
                    Some(i) => self.retap_line(i),
                    None => {
                        self.status = "Click a line in the table first, then press Enter to \
                                       re-tap it."
                            .to_string()
                    }
                }
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
                if let Some(i) = self.edit_target() {
                    self.lines[i].start = None;
                    self.lines[i].sing_end_override = None;
                    self.recompute_next_untimed();
                    self.status = format!("Cleared line {}'s timing (Ctrl+Z to undo).", i + 1);
                }
            }
        }
    }

    /// The line keyboard edits (nudge, re-tap, clear) apply to: the one open
    /// in the fine-tune panel, else the most recently tapped main line.
    fn edit_target(&self) -> Option<usize> {
        if let Some(i) = self.word_tap_line.filter(|&i| i < self.lines.len()) {
            return Some(i);
        }
        let upto = self.next_untimed.min(self.lines.len());
        (0..upto)
            .rev()
            .find(|&i| !self.lines[i].is_backing && self.lines[i].start.is_some())
    }

    /// Moves line `i`'s start by `delta` seconds (the table's -0.1s/+0.1s
    /// and the , / . keys), checked like a typed time so it can't cross
    /// the previous line's end or its own end, then replays from just
    /// before it so you hear the change.
    fn nudge_line_start(&mut self, i: usize, delta: f64) {
        let Some(start) = self.lines.get(i).and_then(|l| l.start) else {
            self.status = format!("Line {} isn't timed yet - tap its start first.", i + 1);
            return;
        };
        let new_start = (start + delta).max(0.0);
        match lyrics::check_start_change(&self.lines, i, new_start) {
            Ok(()) => {
                self.lines[i].start = Some(new_start);
                self.audition_seek(new_start);
                self.status = format!(
                    "Line {} starts at {}.",
                    i + 1,
                    lyrics::format_timecode(new_start)
                );
            }
            Err(e) => self.status = e,
        }
    }

    /// Shifts the selected lines (table checkboxes) by `delta` seconds - the
    /// same as the "Shift selection" buttons, from Shift+, / Shift+.
    fn shift_selection(&mut self, delta: f64) {
        if self.selected_lines.is_empty() {
            self.status = "Select lines with the table's checkboxes first.".to_string();
            return;
        }
        let indices: Vec<usize> = self.selected_lines.iter().copied().collect();
        self.status = match lyrics::shift_lines(&mut self.lines, &indices, delta) {
            Ok(n) => format!(
                "Shifted {n} line(s) {} by {:.2}s.",
                if delta < 0.0 { "earlier" } else { "later" },
                delta.abs()
            ),
            Err(e) => e,
        };
    }

    /// The keyboard-shortcut cheat sheet (F1 / ?), from [`SHORTCUTS`].
    fn draw_shortcuts_window(&mut self, ctx: &egui::Context) {
        if !self.show_shortcuts {
            return;
        }
        let mut open = true;
        egui::Window::new("⌨ Keyboard shortcuts")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Single-key shortcuts work whenever you're not typing in a text box. \
                         \"Current line\" = the line open in the fine-tune panel, else the \
                         one you tapped last.",
                    )
                    .small()
                    .weak(),
                );
                for (group, keys) in SHORTCUTS {
                    ui.add_space(6.0);
                    ui.strong(*group);
                    egui::Grid::new(("shortcuts", *group))
                        .num_columns(2)
                        .spacing([16.0, 3.0])
                        .show(ui, |ui| {
                            for (key, what) in *keys {
                                ui.label(egui::RichText::new(*key).monospace().color(ACCENT));
                                ui.label(*what);
                                ui.end_row();
                            }
                        });
                }
            });
        if !open {
            self.show_shortcuts = false;
        }
    }

    /// Draws the live "what will this look like" preview: a mock TV screen
    /// showing exactly what the exported CDG will show at the current
    /// playback position (or position 0 if nothing is playing). When a
    /// background image/video is set, it's drawn (dimmed) behind everything
    /// else here too, so legibility can be judged without exporting first -
    /// though a background video shows a single representative frame here
    /// rather than true playback (see [`video::extract_video_background_thumbnail`]).
    ///
    /// Fills the available width at 16:9; `max_height`, when given (the
    /// popped-out preview window), also caps it to fit that height.
    /// Hash of everything the `.cdg` export depends on - lyric lines and
    /// their timing, palette, title/artist, timing settings and song length.
    fn cdg_preview_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_vec(&self.lines)
            .unwrap_or_default()
            .hash(&mut h);
        format!("{:?}", self.palette()).hash(&mut h);
        format!("{:?}", self.timing_settings).hash(&mut h);
        format!("{:?}", self.outro).hash(&mut h);
        self.title.trim().hash(&mut h);
        self.artist.trim().hash(&mut h);
        self.audio
            .as_ref()
            .and_then(|a| a.duration())
            .map(f64::to_bits)
            .hash(&mut h);
        h.finish()
    }

    /// Keeps the `.cdg` preview's decoded stream up to date: re-hashes the
    /// inputs at most every [`CDG_PREVIEW_RECHECK`], starts a background
    /// render when they changed (one at a time - a newer change waits for
    /// the running one, then renders again), and installs finished ones.
    /// Returns true while a render is in flight.
    fn update_cdg_preview(&mut self, ctx: &egui::Context) -> bool {
        let now = std::time::Instant::now();
        let due = self
            .cdg_preview
            .last_check
            .is_none_or(|t| now.duration_since(t) >= CDG_PREVIEW_RECHECK);
        if due {
            self.cdg_preview.last_check = Some(now);
            self.cdg_preview.wanted_key = self.cdg_preview_key();
        }
        if let Some((key, rx)) = &self.cdg_preview.job {
            match rx.try_recv() {
                Ok(playback) => {
                    self.cdg_preview.current = Some((*key, playback));
                    self.cdg_preview.shown_pos = None;
                    self.cdg_preview.job = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.cdg_preview.job = None,
            }
        }
        let wanted = self.cdg_preview.wanted_key;
        let have = self.cdg_preview.current.as_ref().map(|(k, _)| *k);
        if self.cdg_preview.job.is_none() && have != Some(wanted) {
            let (timed, total) = self.resolved();
            let backing = self.resolved_backing();
            let palette = self.palette();
            let settings = self.timing_settings;
            let title = (!self.title.trim().is_empty()).then(|| self.title.trim().to_string());
            let artist = (!self.artist.trim().is_empty()).then(|| self.artist.trim().to_string());
            let outro = export::resolve_outro(
                &self.outro,
                &timed,
                &backing,
                total,
                title.is_some() || artist.is_some(),
            );
            let (tx, rx) = std::sync::mpsc::channel();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let bytes = export::render_cdg_with_settings(
                    &timed,
                    &backing,
                    total,
                    &palette,
                    title.as_deref(),
                    artist.as_deref(),
                    &settings,
                    outro.as_ref(),
                );
                let _ = tx.send(cdg_decode::CdgPlayback::new(bytes));
                repaint.request_repaint();
            });
            self.cdg_preview.job = Some((wanted, rx));
        }
        self.cdg_preview.job.is_some()
    }

    /// The preview's ".cdg (exact)" mode: the decoded `.cdg` frame at the
    /// current playback position, scaled up with nearest-neighbor so its
    /// real pixels stay crisp, at the format's own 300:216 shape.
    fn draw_cdg_preview(&mut self, ui: &mut egui::Ui, max_height: Option<f32>) {
        let rendering = self.update_cdg_preview(ui.ctx());
        let aspect = cdg_decode::WIDTH as f32 / cdg_decode::HEIGHT as f32;
        let width = match max_height {
            Some(h) => ui.available_width().min(h * aspect),
            None => ui.available_width(),
        };
        let size = egui::vec2(width, width / aspect);
        let t = self.audio.as_ref().map(|a| a.position()).unwrap_or(0.0);
        if let Some((_, playback)) = &mut self.cdg_preview.current {
            let pos = playback.seek(t);
            if self.cdg_preview.shown_pos != Some(pos) || self.cdg_preview.texture.is_none() {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [cdg_decode::WIDTH, cdg_decode::HEIGHT],
                    &playback.rgba(),
                );
                match &mut self.cdg_preview.texture {
                    Some(tex) => tex.set(image, egui::TextureOptions::NEAREST),
                    None => {
                        self.cdg_preview.texture = Some(ui.ctx().load_texture(
                            "cdg_preview",
                            image,
                            egui::TextureOptions::NEAREST,
                        ))
                    }
                }
                self.cdg_preview.shown_pos = Some(pos);
            }
        }
        match &self.cdg_preview.texture {
            Some(tex) => {
                ui.image((tex.id(), size));
            }
            None => {
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
            }
        }
        if rendering {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("Updating .cdg preview…").small().weak());
            });
        }
    }

    fn draw_preview(&mut self, ui: &mut egui::Ui, max_height: Option<f32>) {
        let toggle_top = ui.cursor().top();
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.preview_mode, PreviewMode::Video, "Video")
                .on_hover_text(
                    "The video export's look: multi-line verse blocks with a smooth wipe.",
                );
            ui.selectable_value(&mut self.preview_mode, PreviewMode::Cdg, ".cdg (exact)")
                .on_hover_text(
                    "The real .cdg output, decoded exactly as a karaoke player shows it - \
                     300x216, 16 colors, one line at a time. Updates a moment after you \
                     change timing, colors or text.",
                );
        });
        let max_height = max_height.map(|h| (h - (ui.cursor().top() - toggle_top)).max(60.0));
        if self.preview_mode == PreviewMode::Cdg {
            self.draw_cdg_preview(ui, max_height);
            return;
        }
        let t = self.audio.as_ref().map(|a| a.position()).unwrap_or(0.0);
        let (timed, total) = self.resolved();
        let backing = self.resolved_backing();
        let card_end = export::title_card_end(&timed);
        let outro = export::resolve_outro(
            &self.outro,
            &timed,
            &backing,
            total,
            !self.title.trim().is_empty() || !self.artist.trim().is_empty(),
        );

        // Fills whatever width it's given (the right panel can be dragged
        // wide), always 16:9. Text sizes below are already proportional to
        // `height`; `s` scales the few remaining fixed pixel sizes (countdown
        // dots, small gaps) and never drops below 1.0, so a preview at or
        // below the panel's default width looks exactly as it always has.
        // 9:16 instead when the video export is set to portrait (Shorts/
        // TikTok/Reels). Sizes scale from the frame's short side and lyric
        // sizes from the lyric size setting, the same as the export (see
        // `video::Canvas::unit`/`lyric_px`).
        let portrait = self.video_style.orientation == video::Orientation::Portrait;
        let aspect = if portrait { 9.0 / 16.0 } else { 16.0 / 9.0 };
        let width = match max_height {
            Some(h) => ui.available_width().min(h * aspect),
            None => ui.available_width(),
        };
        let height = width / aspect;
        let unit = width.min(height);
        let lyric_unit = unit * self.video_style.lyric_scale();
        let s = (unit / PREVIEW_BASE_HEIGHT).max(1.0);

        // The background texture is rendered at the smallest of a few fixed
        // 16:9 resolutions that covers the preview's on-screen pixel width,
        // rather than one that tracks it exactly - so dragging the panel
        // only re-decodes the background when crossing a step, not every
        // frame of the drag.
        let px_width = width * ui.ctx().pixels_per_point();
        let bg_width = PREVIEW_BG_WIDTHS
            .iter()
            .copied()
            .find(|&w| w as f32 >= px_width)
            .unwrap_or(PREVIEW_BG_WIDTHS[PREVIEW_BG_WIDTHS.len() - 1]);
        let bg_height = if portrait {
            bg_width * 16 / 9
        } else {
            bg_width * 9 / 16
        };
        self.ensure_background_preview(ui.ctx(), bg_width, bg_height);

        let (rect, _response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        match self
            .background_preview
            .as_ref()
            .and_then(|(_, _, _, t)| t.as_ref())
        {
            Some(texture) => {
                ui.painter().image(
                    texture.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            None => {
                ui.painter().rect_filled(rect, 0.0, self.color_bg);
            }
        }
        if self.background.is_some() {
            let alpha = (self.background_dim.clamp(0.0, 1.0) * 255.0).round() as u8;
            ui.painter()
                .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(alpha));
        }

        let mut content_ui = ui.child_ui(rect, egui::Layout::top_down(egui::Align::Center), None);
        // Safety net alongside the proportional sizing below: nothing drawn
        // in here can spill past the visible mock-screen rect into the rest
        // of the page, even in an edge case the proportional math doesn't
        // quite cover (an unusually tall custom font's own line metrics,
        // very long lines that wrap to extra rows, ...).
        content_ui.set_clip_rect(rect);
        let ui = &mut content_ui;
        {
            let legend_singers = singer_legend(&timed, &backing);
            let has_title_card = !self.title.trim().is_empty()
                || !self.artist.trim().is_empty()
                || !legend_singers.is_empty();

            ui.vertical_centered(|ui| {
                ui.add_space(height * 0.15);

                // The outro card, matching `video::draw_outro`: its lines
                // (the first larger), then title / "by artist", fading in.
                if let Some(o) = outro.as_ref().filter(|o| t >= o.start) {
                    let alpha = ((t - o.start) / 0.6).clamp(0.0, 1.0) as f32;
                    ui.add_space(height * 0.1);
                    for (k, line) in o.lines.iter().enumerate() {
                        let text = egui::RichText::new(line);
                        let text = if k == 0 {
                            text.size(unit * 0.08).strong()
                        } else {
                            text.size(unit * 0.05)
                        };
                        ui.colored_label(self.color_title.gamma_multiply(alpha), text);
                    }
                    if o.show_title {
                        if !self.title.trim().is_empty() {
                            ui.colored_label(
                                self.color_title.gamma_multiply(alpha),
                                egui::RichText::new(self.title.trim())
                                    .size(unit * 0.06)
                                    .strong(),
                            );
                        }
                        if !self.artist.trim().is_empty() {
                            ui.colored_label(
                                self.color_artist.gamma_multiply(alpha),
                                egui::RichText::new(format!("by {}", self.artist.trim()))
                                    .size(unit * 0.045),
                            );
                        }
                    }
                    return;
                }

                if has_title_card && t < card_end {
                    // Every size in this function is a fraction of `height`
                    // (the preview rect's own pixel height), matching
                    // video.rs's render_frame ratios exactly - not the
                    // fixed pixel sizes this used to have, which were sized
                    // for the small preview panel in isolation rather than
                    // scaled to it, and could overflow the visible mock
                    // screen for a custom font with taller natural line
                    // metrics than the default one they were tuned against.
                    if !self.title.trim().is_empty() {
                        ui.colored_label(
                            self.color_title,
                            egui::RichText::new(&self.title).size(unit * 0.10).strong(),
                        );
                    }
                    if !self.artist.trim().is_empty() {
                        ui.colored_label(
                            self.color_artist,
                            egui::RichText::new(format!("by {}", self.artist)).size(unit * 0.05),
                        );
                    }
                    if !legend_singers.is_empty() {
                        ui.add_space(6.0 * s);
                        // Built as a single multi-colored label (rather than a
                        // `ui.horizontal` of separate labels) so
                        // `vertical_centered` centers the whole legend as one
                        // block, the same way it centers the title/artist text.
                        let mut job = egui::text::LayoutJob::default();
                        for (i, (singer, custom_name)) in legend_singers.iter().enumerate() {
                            if i > 0 {
                                job.append("   ", 0.0, egui::TextFormat::default());
                            }
                            // Unsung (upcoming), not highlight - this legend
                            // appears before anything has been sung, so the
                            // highlight color (only ever seen once a line is
                            // already playing) isn't what's actually about to
                            // be shown.
                            let (unsung, _) = self.singer_colors(*singer);
                            job.append(
                                lyrics::effective_singer_label(*singer, custom_name),
                                0.0,
                                egui::TextFormat {
                                    color: unsung,
                                    font_id: egui::FontId::proportional(unit * 0.04),
                                    ..Default::default()
                                },
                            );
                        }
                        ui.label(job);
                    }
                    if self.show_credit_line {
                        ui.add_space(10.0 * s);
                        ui.colored_label(
                            self.color_preview,
                            egui::RichText::new(video::CREDIT_TEXT).size(unit * 0.028),
                        );
                    }
                    return;
                }

                // Between the title card fading (or song start, if there's
                // no title card) and the first lyric line actually
                // starting: show the same "get ready" countdown used for
                // mid-song breaks and the exported files, instead of
                // sitting on the generic "nothing timed yet" note icon.
                if let Some(first) = timed.first() {
                    if t < first.start {
                        if let Some((cd_start, cd_end)) = lyrics::countdown_window_between(
                            card_end,
                            first.start,
                            first.countdown_mode,
                            &self.timing_settings,
                        ) {
                            if t >= cd_start {
                                let frac = ((t - cd_start) / (cd_end - cd_start).max(0.001))
                                    .clamp(0.0, 1.0);
                                let lit = ((frac * 4.0).floor() as i32 + 1).clamp(0, 4) as usize;
                                let (_, highlight) = self.singer_colors(first.singer);
                                ui.add_space(10.0 * s);
                                ui.horizontal(|ui| {
                                    ui.add_space(width / 2.0 - 40.0 * s);
                                    for dot in 0..4 {
                                        let color = if dot < lit {
                                            highlight
                                        } else {
                                            self.color_preview
                                        };
                                        let (rect, _) = ui.allocate_exact_size(
                                            egui::vec2(14.0 * s, 14.0 * s),
                                            egui::Sense::hover(),
                                        );
                                        ui.painter().circle_filled(rect.center(), 6.0 * s, color);
                                        ui.add_space(8.0 * s);
                                    }
                                });

                                // Reveal the first block's lines too, same
                                // idea as the mid-song countdown's reveal -
                                // see the matching comment in video.rs's
                                // render_frame.
                                let blocks = group_into_blocks(&timed);
                                let block = blocks
                                    .iter()
                                    .find(|b| b.contains(&0))
                                    .cloned()
                                    .unwrap_or_else(|| vec![0]);
                                let lyric_family = self.lyric_font_family();
                                let reveal_alpha = lyrics::reveal_fade_alpha(cd_start, t);
                                ui.add_space(10.0 * s);
                                if reveal_alpha > 0.0 {
                                    for &idx in &block {
                                        let line = &timed[idx];
                                        let (unsung, _) = self.singer_colors(line.singer);
                                        let normalized = lyrics::normalize_text(&line.text);
                                        ui.colored_label(
                                            faded_color(unsung, reveal_alpha),
                                            egui::RichText::new(normalized)
                                                .size(lyric_unit * 0.055)
                                                .family(lyric_family.clone()),
                                        );
                                    }
                                }
                            }
                        }
                        return;
                    }
                }

                match timed
                    .iter()
                    .enumerate()
                    .find(|(_, l)| t >= l.start && t < l.end)
                {
                    Some((current_idx, _)) => {
                        // Show the whole verse block the current line belongs
                        // to - like a real karaoke video's verse block, not
                        // just the current line - so the preview matches what
                        // the video/CDG exporters actually display.
                        let blocks = group_into_blocks(&timed);
                        let block = blocks
                            .iter()
                            .find(|b| b.contains(&current_idx))
                            .cloned()
                            .unwrap_or_else(|| vec![current_idx]);
                        let slot_in_block =
                            block.iter().position(|&i| i == current_idx).unwrap_or(0);

                        // Once the current line is done being sung and there's a real
                        // musical break before the next one (long enough to warrant the
                        // countdown dots below), hide the not-yet-started lines in this
                        // block instead of leaving them on screen the whole time - it
                        // should read as "done, waiting", then, once the countdown
                        // itself starts, both the dots *and* the upcoming line(s)
                        // appear together (drawn above the block, not below, so the
                        // dots read as "counting down to the words shown right under
                        // them") so the singer can read ahead during the countdown
                        // rather than only right as the words go live - see the
                        // matching comment in video.rs's render_frame.
                        let current_line = &timed[current_idx];
                        let next_countdown_mode = timed
                            .get(current_idx + 1)
                            .map(|n| n.countdown_mode)
                            .unwrap_or_default();
                        let cd_window = countdown_window(
                            current_line,
                            next_countdown_mode,
                            &self.timing_settings,
                        );
                        let hide_upcoming_base = hide_upcoming_lines(
                            current_line,
                            t,
                            next_countdown_mode,
                            &self.timing_settings,
                        );
                        // `Some(cd_start)` only once the countdown that
                        // reveals this block's upcoming lines has actually
                        // started - used both to stop hiding them (below)
                        // and, distinctly, to fade them in starting from
                        // that exact moment - see the matching comment in
                        // video.rs's render_frame.
                        let revealed_at =
                            cd_window.and_then(|(cd_start, _)| (t >= cd_start).then_some(cd_start));
                        let hide_upcoming = hide_upcoming_base && revealed_at.is_none();
                        let lyric_family = self.lyric_font_family();

                        // Only draw the countdown if there's a real next line to
                        // count into - see the matching comment in video.rs's
                        // render_frame. Drawn *before* the block below (not after),
                        // so it appears above the upcoming line(s) it's counting
                        // down to, matching the top-anchored layout this looks to
                        // approximate the placement of.
                        let has_next_line = current_idx + 1 < timed.len();
                        if let Some((cd_start, cd_end)) = cd_window {
                            if has_next_line && t >= cd_start {
                                let frac = ((t - cd_start) / (cd_end - cd_start).max(0.001))
                                    .clamp(0.0, 1.0);
                                let lit = ((frac * 4.0).floor() as i32 + 1).clamp(0, 4) as usize;
                                let next_singer = timed
                                    .get(current_idx + 1)
                                    .map(|l| l.singer)
                                    .unwrap_or(current_line.singer);
                                let (_, next_highlight) = self.singer_colors(next_singer);
                                ui.horizontal(|ui| {
                                    ui.add_space(width / 2.0 - 40.0 * s);
                                    for dot in 0..4 {
                                        let color = if dot < lit {
                                            next_highlight
                                        } else {
                                            self.color_preview
                                        };
                                        let (rect, _) = ui.allocate_exact_size(
                                            egui::vec2(14.0 * s, 14.0 * s),
                                            egui::Sense::hover(),
                                        );
                                        ui.painter().circle_filled(rect.center(), 6.0 * s, color);
                                        ui.add_space(8.0 * s);
                                    }
                                });
                                ui.add_space(10.0 * s);

                                // A real gap almost always falls at a
                                // verse/chorus boundary, so the line being
                                // counted into usually starts a *different*
                                // on-screen block than `current_idx`'s own -
                                // meaning there's nothing in `block` (below)
                                // for the per-slot loop to reveal (every
                                // line in it has already faded out by now
                                // either way, per `sung_line_fade_alpha`).
                                // Preview the *next* block directly instead,
                                // same idea as the intro countdown
                                // previewing the very first one - see the
                                // matching comment in video.rs's
                                // render_frame.
                                let next_idx = current_idx + 1;
                                if !block.contains(&next_idx) {
                                    let next_block = blocks
                                        .iter()
                                        .find(|b| b.contains(&next_idx))
                                        .cloned()
                                        .unwrap_or_else(|| vec![next_idx]);
                                    let reveal_alpha = lyrics::reveal_fade_alpha(cd_start, t);
                                    if reveal_alpha > 0.0 {
                                        for &idx in &next_block {
                                            let line = &timed[idx];
                                            let (unsung, _) = self.singer_colors(line.singer);
                                            let normalized = lyrics::normalize_text(&line.text);
                                            ui.colored_label(
                                                faded_color(unsung, reveal_alpha),
                                                egui::RichText::new(normalized)
                                                    .size(lyric_unit * 0.055)
                                                    .family(lyric_family.clone()),
                                            );
                                        }
                                    }
                                    return;
                                }
                            }
                        }

                        // Once a block's leading slots have fully faded (top
                        // to bottom, since `slot_fade_alpha` is monotonic
                        // down a block), roll in a preview of whatever comes
                        // right after this block into those now-empty rows
                        // instead of leaving them blank until the next real
                        // line finally starts - a continuous, KaraFun-style
                        // hand-off rather than a hard cut, as long as the gap
                        // to what's rolled in is short enough that a
                        // countdown wouldn't trigger for it anyway (a real
                        // countdown-worthy gap is handled by the dedicated
                        // reveal logic above instead, not blended with this).
                        // See the matching comment in video.rs's render_frame.
                        let freed_count = (0..=slot_in_block)
                            .take_while(|&s| lyrics::slot_fade_alpha(&timed[block[s]], t) <= 0.0)
                            .count();
                        let rolling = lyrics::rolling_preview_lines(
                            &timed,
                            &block,
                            freed_count,
                            &self.timing_settings,
                        );

                        for (slot, &idx) in block.iter().enumerate() {
                            if slot < freed_count {
                                if let Some(&roll_idx) = rolling.get(slot) {
                                    let roll_line = &timed[roll_idx];
                                    let (roll_unsung, _) = self.singer_colors(roll_line.singer);
                                    let roll_normalized = lyrics::normalize_text(&roll_line.text);
                                    ui.colored_label(
                                        roll_unsung,
                                        egui::RichText::new(roll_normalized)
                                            .size(lyric_unit * 0.055)
                                            .family(lyric_family.clone()),
                                    );
                                } else {
                                    ui.add_space(lyric_unit * 0.11);
                                }
                                continue;
                            }
                            let line = &timed[idx];
                            let (unsung, highlight) = self.singer_colors(line.singer);
                            let normalized = lyrics::normalize_text(&line.text);
                            match slot.cmp(&slot_in_block) {
                                std::cmp::Ordering::Less => {
                                    // Already sung - full highlight color for a
                                    // while, then fades out on its own timer
                                    // (see `sung_line_fade_alpha`) rather than
                                    // the whole block blanking at once - each
                                    // line fades independently, top to bottom,
                                    // as its own linger runs out.
                                    let alpha = lyrics::sung_line_fade_alpha(line.sing_end, t);
                                    if alpha > 0.0 {
                                        ui.colored_label(
                                            faded_color(highlight, alpha),
                                            egui::RichText::new(normalized)
                                                .size(lyric_unit * 0.055)
                                                .family(lyric_family.clone()),
                                        );
                                    } else {
                                        ui.add_space(lyric_unit * 0.11);
                                    }
                                }
                                std::cmp::Ordering::Equal => {
                                    if t < line.sing_end {
                                        let mut job = egui::text::LayoutJob {
                                            halign: egui::Align::Center,
                                            ..Default::default()
                                        };
                                        // Same continuous per-line fraction the video/CDG
                                        // exporters use, so the wipe moves smoothly through a
                                        // word's letters as it's held out instead of the whole
                                        // word snapping to `highlight` the instant its
                                        // timestamp is reached (which looked instantaneous for
                                        // long-held words and words with few characters alike).
                                        let chars: Vec<char> = normalized.chars().collect();
                                        let spans = lyrics::word_char_spans(&line.text);
                                        let boundary = lyrics::current_line_wipe_fraction(line, t)
                                            * chars.len().max(1) as f32;
                                        let font_id = egui::FontId::new(
                                            lyric_unit * 0.055,
                                            lyric_family.clone(),
                                        );
                                        for (i, &(offset, len)) in spans.iter().enumerate() {
                                            let word_text: String =
                                                chars[offset..offset + len].iter().collect();
                                            let split = ((boundary - offset as f32)
                                                .round()
                                                .clamp(0.0, len as f32))
                                                as usize;
                                            let sung_part: String =
                                                word_text.chars().take(split).collect();
                                            let rest: String =
                                                word_text.chars().skip(split).collect();
                                            let suffix = if i + 1 < spans.len() { " " } else { "" };
                                            let append =
                                                |ui_job: &mut egui::text::LayoutJob,
                                                 text: &str,
                                                 color: egui::Color32| {
                                                    if text.is_empty() {
                                                        return;
                                                    }
                                                    ui_job.append(
                                                        text,
                                                        0.0,
                                                        egui::TextFormat {
                                                            color,
                                                            font_id: font_id.clone(),
                                                            ..Default::default()
                                                        },
                                                    );
                                                };
                                            append(&mut job, &sung_part, highlight);
                                            append(&mut job, &format!("{rest}{suffix}"), unsung);
                                        }
                                        ui.label(job);
                                    } else {
                                        // Just finished - same fade-out treatment as an
                                        // already-sung line above, using this line's own
                                        // sing_end (it only just became "already sung" itself).
                                        let alpha = lyrics::sung_line_fade_alpha(line.sing_end, t);
                                        if alpha > 0.0 {
                                            ui.colored_label(
                                                faded_color(highlight, alpha),
                                                egui::RichText::new(normalized)
                                                    .size(lyric_unit * 0.055)
                                                    .family(lyric_family.clone()),
                                            );
                                        } else {
                                            ui.add_space(lyric_unit * 0.11);
                                        }
                                    }
                                }
                                std::cmp::Ordering::Greater => {
                                    if hide_upcoming {
                                        ui.add_space(lyric_unit * 0.11);
                                    } else {
                                        // Fading in only applies to a line that was
                                        // actually hidden and just got revealed by the
                                        // countdown starting (`revealed_at`) - a line
                                        // that was never hidden in the first place (a
                                        // gap too short to warrant a countdown at all)
                                        // has always been fully visible and shouldn't
                                        // suddenly gain a fade-in.
                                        let alpha = match revealed_at {
                                            Some(reveal_at) if hide_upcoming_base => {
                                                lyrics::reveal_fade_alpha(reveal_at, t)
                                            }
                                            _ => 1.0,
                                        };
                                        if alpha > 0.0 {
                                            ui.colored_label(
                                                faded_color(unsung, alpha),
                                                egui::RichText::new(normalized)
                                                    .size(lyric_unit * 0.055)
                                                    .family(lyric_family.clone()),
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        ui.add_space(8.0 * s);
                    }
                    None => {
                        ui.label(
                            egui::RichText::new("♪")
                                .size(28.0 * s)
                                .color(self.color_preview),
                        );
                    }
                }
            });
        }

        // The backing track's own lane, at the same fixed spot near the
        // bottom the video export uses (see `video::BACKING_LANE_Y`) -
        // painted directly rather than laid out with the flow above, since
        // it's independent of whatever the main lines are doing.
        let windows = lyrics::backing_display_windows(&backing);
        if let Some(k) = lyrics::active_backing_line(&windows, t) {
            let line = &backing[k];
            let (unsung, highlight) = self.singer_colors(line.singer);
            let font_id = egui::FontId::new(
                lyric_unit * video::BACKING_LANE_FONT,
                self.lyric_font_family(),
            );
            let normalized = lyrics::normalize_text(&line.text);
            let job = if t >= line.start && t < line.sing_end {
                wipe_layout_job(
                    &normalized,
                    lyrics::current_line_wipe_fraction(line, t),
                    font_id,
                    unsung,
                    highlight,
                )
            } else {
                let color = if t >= line.sing_end {
                    highlight
                } else {
                    unsung
                };
                let alpha = lyrics::backing_line_alpha(line, windows[k], t);
                let mut job = egui::text::LayoutJob::default();
                job.append(
                    &normalized,
                    0.0,
                    egui::TextFormat {
                        color: faded_color(color, alpha),
                        font_id,
                        ..Default::default()
                    },
                );
                job
            };
            let mut job = job;
            job.wrap.max_width = rect.width() * 0.92;
            job.halign = egui::Align::Center;
            let galley = ui.fonts(|f| f.layout_job(job));
            let baseline_y = rect.top() + height * video::BACKING_LANE_Y;
            let pos = egui::pos2(rect.center().x, baseline_y - galley.size().y * 0.8);
            ui.painter()
                .with_clip_rect(rect)
                .galley(pos, galley, egui::Color32::WHITE);
        }
    }

    /// Draws the fine-tuning timeline: one draggable/resizable "bubble" per
    /// timed line, spanning its sung window `[start, sing_end)` - a
    /// kdenlive/karaodeo-style clip track. Dragging a bubble's body moves
    /// `start` (preserving its duration); dragging its left/right edge
    /// trims `start`/`sing_end` independently. Clicking a line's bubble
    /// also expands it into a second row of per-word bubbles underneath
    /// (spanning each word's `[highlight_at, held_until)`), with the same
    /// drag behavior, for word-level fine-tuning without leaving the
    /// timeline. This writes the exact same `LyricLine` fields the
    /// tap-based workflow and the "Fine-tune words" panel do, so all three
    /// stay in sync automatically.
    fn draw_timeline(&mut self, ui: &mut egui::Ui) {
        let (main_timed, main_indices) = self.resolved_with_indices();
        let (backing_timed, backing_indices) =
            lyrics::resolve_backing_track_indexed(&self.lines, &self.timing_settings);
        let has_backing = !backing_timed.is_empty();
        let area_width = ui.available_width();

        ui.horizontal(|ui| {
            ui.label("Timeline:");
            let visible_secs = (area_width / self.timeline_view.px_per_sec) as f64;
            let mid_time = self.timeline_view.scroll_secs + visible_secs / 2.0;
            if ui.small_button("−").clicked() {
                self.timeline_view.zoom_at(1.0 / 1.5, 0.0, mid_time);
            }
            if ui.small_button("+").clicked() {
                self.timeline_view.zoom_at(1.5, 0.0, mid_time);
            }
            if ui.small_button("Fit").clicked() {
                let last_end = main_timed
                    .last()
                    .map(|t| t.end)
                    .unwrap_or(30.0)
                    .max(backing_timed.iter().map(|t| t.sing_end).fold(0.0, f64::max))
                    .max(1.0);
                self.timeline_view.px_per_sec = (area_width / last_end as f32)
                    .clamp(timeline::MIN_PX_PER_SEC, timeline::MAX_PX_PER_SEC);
                self.timeline_view.scroll_secs = 0.0;
            }
            help_text(
                ui,
                self.prefs.show_tips,
                "Drag bubbles · scroll to zoom · Shift+scroll to pan.",
                "Drag a bubble to move it, its edges to trim start/end. Click or drag \
                 empty space to seek/scrub playback - scroll to zoom, shift+scroll to pan.",
            );
            if self.waveform_job.is_some() {
                ui.label(egui::RichText::new("(building waveform…)").weak());
            }
        });

        if main_timed.is_empty() && !has_backing {
            ui.label(
                "Time at least one line (tap along, or use the table below) to use the timeline.",
            );
            return;
        }

        let ruler_h = 16.0;
        let line_row_h = 28.0;
        let row_gap = 6.0;
        let word_row_h = 28.0;
        // Each track (main, and the backing track if there is one) gets its
        // own lane: a line-bubble row with a word-bubble row under it.
        let lane_h = line_row_h + row_gap + word_row_h;
        let lane_gap = 10.0;
        let backing_lane_h = if has_backing { lane_gap + lane_h } else { 0.0 };
        let height = ruler_h + 4.0 + lane_h + backing_lane_h + 4.0;
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(area_width, height),
            egui::Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);

        // Plain scroll zooms; shift+scroll pans instead (handled after the
        // bubble loops, once we know whether a drag is in progress).
        if response.hovered() && !ui.input(|i| i.modifiers.shift) {
            let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll_y != 0.0 {
                if let Some(pos) = response.hover_pos() {
                    let anchor_time = self.timeline_view.x_to_time(rect.left(), pos.x);
                    let factor = 1.0 + scroll_y * 0.002;
                    self.timeline_view.zoom_at(factor, rect.left(), anchor_time);
                }
            }
        }

        // Time ruler - shared by both rows since they're on the same axis.
        let interval = timeline::tick_interval_secs(self.timeline_view.px_per_sec, 50.0);
        let visible_secs = (rect.width() / self.timeline_view.px_per_sec) as f64;
        let mut t = (self.timeline_view.scroll_secs / interval).floor() * interval;
        let text_color = ui.visuals().weak_text_color();
        while t <= self.timeline_view.scroll_secs + visible_secs + interval {
            let x = self.timeline_view.time_to_x(rect.left(), t);
            if (rect.left()..=rect.right()).contains(&x) {
                painter.line_segment(
                    [
                        egui::pos2(x, rect.top()),
                        egui::pos2(x, rect.top() + ruler_h),
                    ],
                    egui::Stroke::new(1.0_f32, text_color),
                );
                painter.text(
                    egui::pos2(x + 2.0, rect.top()),
                    egui::Align2::LEFT_TOP,
                    format_time(t.max(0.0)),
                    egui::FontId::proportional(10.0),
                    text_color,
                );
            }
            t += interval;
        }

        // Waveform backdrop, spanning both bubble rows - drawn before the
        // bubbles (and behind their semi-transparent fill, see below) so a
        // bubble's edge can be visually compared against an actual vocal
        // onset instead of just trusting the tapped/estimated timestamp.
        let main_lane_top = rect.top() + ruler_h + 4.0;
        if let Some(waveform) = &self.waveform {
            let wave_top = main_lane_top;
            let wave_bottom = main_lane_top + lane_h + backing_lane_h;
            let wave_center = (wave_top + wave_bottom) / 2.0;
            let wave_half_h = (wave_bottom - wave_top) / 2.0 - 2.0;
            let wc = ui.visuals().weak_text_color();
            let wave_color = egui::Color32::from_rgba_unmultiplied(wc.r(), wc.g(), wc.b(), 150);
            let mut x = rect.left();
            while x < rect.right() {
                let t0 = self.timeline_view.x_to_time(rect.left(), x);
                let t1 = self.timeline_view.x_to_time(rect.left(), x + 1.0);
                if let Some((lo, hi)) = waveform.peak_in_range(t0, t1) {
                    let y0 = wave_center - hi.clamp(-1.0, 1.0) * wave_half_h;
                    let y1 = wave_center - lo.clamp(-1.0, 1.0) * wave_half_h;
                    painter.line_segment(
                        [egui::pos2(x, y0.min(y1)), egui::pos2(x, y1.max(y0))],
                        egui::Stroke::new(1.0_f32, wave_color),
                    );
                }
                x += 1.0;
            }
        }

        let backing_lane_top = main_lane_top + lane_h + lane_gap;
        if has_backing {
            painter.line_segment(
                [
                    egui::pos2(rect.left(), backing_lane_top - lane_gap / 2.0),
                    egui::pos2(rect.right(), backing_lane_top - lane_gap / 2.0),
                ],
                egui::Stroke::new(1.0_f32, ui.visuals().weak_text_color().gamma_multiply(0.5)),
            );
            painter.text(
                egui::pos2(rect.left() + 2.0, backing_lane_top - lane_gap / 2.0),
                egui::Align2::LEFT_CENTER,
                "Backing",
                egui::FontId::proportional(9.0),
                ui.visuals().weak_text_color(),
            );
        }
        let lanes: [(&[TimedLine], &[usize], f32, bool); 2] = [
            (&main_timed, &main_indices, main_lane_top, false),
            (&backing_timed, &backing_indices, backing_lane_top, true),
        ];
        for (timed, indices, line_row_top, is_backing_lane) in lanes {
            // Line bubbles - one per timed line, in start-time order. A backing
            // lane's lines overlap each other and the main lane freely, so its
            // drags have no neighbor bounds at all.
            for (i, line) in timed.iter().enumerate() {
                let orig_idx = indices[i];
                let x0 = self.timeline_view.time_to_x(rect.left(), line.start);
                let x1 = self.timeline_view.time_to_x(rect.left(), line.sing_end);
                if x1 < rect.left() || x0 > rect.right() {
                    continue; // fully off-screen - skip drawing and interaction
                }
                let bubble_rect = egui::Rect::from_min_max(
                    egui::pos2(x0.max(rect.left()), line_row_top),
                    egui::pos2(
                        x1.max(x0 + 1.0).min(rect.right()),
                        line_row_top + line_row_h,
                    ),
                );

                let id = ui.id().with(("timeline_bubble", orig_idx));
                let bubble_response = ui.interact(bubble_rect, id, egui::Sense::click_and_drag());

                let (_, highlight) = self.singer_colors(line.singer);
                let is_selected = self.word_tap_line == Some(orig_idx);
                let fill = highlight.gamma_multiply(if is_selected { 0.85 } else { 0.55 });
                // Real alpha (not just a darker color) so the waveform drawn
                // behind bubbles is still visible through them - otherwise a
                // misaligned bubble would hide the very peak it should be
                // lined up against.
                let fill = egui::Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), 205);
                let stroke = if is_selected {
                    egui::Stroke::new(2.0_f32, egui::Color32::WHITE)
                } else {
                    egui::Stroke::new(1.0_f32, highlight)
                };
                painter.rect_filled(bubble_rect, 3.0, fill);
                painter.rect_stroke(bubble_rect, 3.0, stroke);
                Self::draw_edge_grab_strips(&painter, bubble_rect);
                if bubble_rect.width() > 16.0 {
                    painter.with_clip_rect(bubble_rect.shrink(2.0)).text(
                        egui::pos2(bubble_rect.left() + 4.0, bubble_rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        &line.text,
                        egui::FontId::proportional(12.0),
                        egui::Color32::WHITE,
                    );
                }

                let active_mode = self
                    .timeline_drag
                    .as_ref()
                    .filter(|d| d.line_idx == orig_idx && d.target == TimelineDragTarget::Line)
                    .map(|d| d.session.mode);
                if let Some(icon) = timeline_cursor_icon(active_mode, &bubble_response, bubble_rect)
                {
                    ui.ctx().set_cursor_icon(icon);
                }

                if bubble_response.clicked() {
                    self.word_tap_line = Some(orig_idx);
                }

                if bubble_response.drag_started() {
                    // Deliberately *not* `interact_pointer_pos()` here - egui
                    // only recognizes a drag once the pointer has moved past
                    // its own click-vs-drag threshold (`MAX_CLICK_DIST`, 6px as
                    // of egui 0.28), so by the time `drag_started()` fires,
                    // `interact_pointer_pos()` already reflects that post-
                    // threshold position, shifted a few pixels in whatever
                    // direction the user's hand first moved. Classifying the
                    // drag mode from that shifted position - rather than the
                    // actual mouse-down point - was misclassifying an edge-grab
                    // as a whole-bubble move whenever the very first motion
                    // happened to be toward the bubble's interior (exactly the
                    // natural direction when shrinking via that edge), with
                    // "drag the other way first, then back" as the only
                    // workaround. `press_origin()` is the true mouse-down
                    // position, unaffected by that threshold.
                    if let Some(press_pos) = ui.input(|i| i.pointer.press_origin()) {
                        let local_x = press_pos.x - bubble_rect.left();
                        let mode = timeline::classify_drag(local_x, bubble_rect.width());
                        let prev_sing_end = if i > 0 && !is_backing_lane {
                            Some(timed[i - 1].sing_end)
                        } else {
                            None
                        };
                        let next_start = timed
                            .get(i + 1)
                            .filter(|_| !is_backing_lane)
                            .map(|t| t.start);
                        let bounds = timeline::drag_bounds(prev_sing_end, next_start);
                        // Trimming the left edge must keep the right edge fixed -
                        // if the end isn't already pinned, pin it now so the
                        // automatic estimate doesn't shift out from under us as
                        // `start` changes.
                        if mode == timeline::DragMode::LeftEdge {
                            if let Some(l) = self.lines.get_mut(orig_idx) {
                                if l.sing_end_override.is_none() {
                                    l.sing_end_override = Some(line.sing_end);
                                }
                            }
                        }
                        self.timeline_drag = Some(TimelineDrag {
                            line_idx: orig_idx,
                            target: TimelineDragTarget::Line,
                            session: timeline::DragSession::start(
                                mode,
                                line.start,
                                line.sing_end,
                                bounds,
                                press_pos.x,
                            ),
                        });
                    }
                }

                if bubble_response.dragged() {
                    if let Some(pos) = bubble_response.interact_pointer_pos() {
                        let resolved = self
                            .timeline_drag
                            .as_ref()
                            .filter(|d| {
                                d.line_idx == orig_idx && d.target == TimelineDragTarget::Line
                            })
                            .map(|d| d.session.resolve(pos.x, self.timeline_view.px_per_sec));
                        if let Some((new_start, new_sing_end)) = resolved {
                            if let Some(l) = self.lines.get_mut(orig_idx) {
                                if let Some(s) = new_start {
                                    l.start = Some(s);
                                }
                                if let Some(e) = new_sing_end {
                                    l.sing_end_override = Some(e);
                                }
                            }
                        }
                    }
                }

                if bubble_response.drag_stopped()
                    && self.timeline_drag.as_ref().map(|d| (d.line_idx, d.target))
                        == Some((orig_idx, TimelineDragTarget::Line))
                {
                    if let Some(mode) = self.timeline_drag.as_ref().map(|d| d.session.mode) {
                        let seek_near = match mode {
                            timeline::DragMode::RightEdge => self.lines[orig_idx]
                                .sing_end_override
                                .unwrap_or(line.sing_end),
                            timeline::DragMode::Body | timeline::DragMode::LeftEdge => {
                                self.lines[orig_idx].start.unwrap_or(line.start)
                            }
                        };
                        self.audition_seek(seek_near);
                    }
                    self.timeline_drag = None;
                }
            }

            // Word bubbles for *every* line, not just a selected one - drawn
            // the whole time, aligned under each line's own span in the row
            // below, so fine-tuning individual words never requires first
            // clicking a line's bubble to "activate" it.
            let word_row_top = line_row_top + line_row_h + row_gap;
            for (i, line) in timed.iter().enumerate() {
                let orig_idx = indices[i];
                // Quick reject on the line's own [start, end) span (its words'
                // `held_until` can extend as far as `end` - see the first/last
                // word special-casing below) before bothering to compute word
                // timings for a line that's entirely off-screen anyway.
                let line_x0 = self.timeline_view.time_to_x(rect.left(), line.start);
                let line_x1 = self.timeline_view.time_to_x(rect.left(), line.end);
                if line_x1 < rect.left() || line_x0 > rect.right() {
                    continue;
                }
                let words = lyrics::word_timings(line);
                let prev_line_sing_end = if i > 0 && !is_backing_lane {
                    Some(timed[i - 1].sing_end)
                } else {
                    None
                };
                let last_word_idx = words.len().saturating_sub(1);
                for (w, word) in words.iter().enumerate() {
                    let x0 = self.timeline_view.time_to_x(rect.left(), word.highlight_at);
                    let x1 = self.timeline_view.time_to_x(rect.left(), word.held_until);
                    if x1 < rect.left() || x0 > rect.right() {
                        continue;
                    }
                    let bubble_rect = egui::Rect::from_min_max(
                        egui::pos2(x0.max(rect.left()), word_row_top),
                        egui::pos2(
                            x1.max(x0 + 1.0).min(rect.right()),
                            word_row_top + word_row_h,
                        ),
                    );

                    let id = ui.id().with(("timeline_word_bubble", orig_idx, w));
                    let bubble_response =
                        ui.interact(bubble_rect, id, egui::Sense::click_and_drag());

                    let (_, highlight) = self.singer_colors(line.singer);
                    let word_fill = highlight.gamma_multiply(0.7);
                    let word_fill = egui::Color32::from_rgba_unmultiplied(
                        word_fill.r(),
                        word_fill.g(),
                        word_fill.b(),
                        205,
                    );
                    painter.rect_filled(bubble_rect, 3.0, word_fill);
                    painter.rect_stroke(
                        bubble_rect,
                        3.0,
                        egui::Stroke::new(1.0_f32, egui::Color32::WHITE.gamma_multiply(0.5)),
                    );
                    Self::draw_edge_grab_strips(&painter, bubble_rect);
                    if bubble_rect.width() > 14.0 {
                        painter.with_clip_rect(bubble_rect.shrink(2.0)).text(
                            egui::pos2(bubble_rect.left() + 3.0, bubble_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            word.text,
                            egui::FontId::proportional(11.0),
                            egui::Color32::WHITE,
                        );
                    }

                    let target = TimelineDragTarget::Word(w);
                    let active_mode = self
                        .timeline_drag
                        .as_ref()
                        .filter(|d| d.line_idx == orig_idx && d.target == target)
                        .map(|d| d.session.mode);
                    if let Some(icon) =
                        timeline_cursor_icon(active_mode, &bubble_response, bubble_rect)
                    {
                        ui.ctx().set_cursor_icon(icon);
                    }

                    if bubble_response.drag_started() {
                        // See the matching comment on the line-bubble drag
                        // above - `press_origin()`, not `interact_pointer_pos()`,
                        // is what the drag mode must be classified from.
                        if let Some(press_pos) = ui.input(|i| i.pointer.press_origin()) {
                            let local_x = press_pos.x - bubble_rect.left();
                            let mode = timeline::classify_drag(local_x, bubble_rect.width());
                            // Bound by the *line's* own singing window, not
                            // the immediate neighbor's position - words
                            // default to touching edge-to-edge with zero
                            // gap, so bounding a drag by a neighbor's
                            // current position would leave zero room to
                            // move (a body-drag's available range is its
                            // *current* span subtracted from the bound, and
                            // that span already exactly fills the gap to a
                            // touching neighbor). Letting a word's bubble
                            // freely overlap a neighbor's default position is
                            // harmless for the wipe rendering, which always
                            // hands off at the *next* word's own start
                            // regardless of a dragged word's start/end - the
                            // one thing worth keeping in mind is that this
                            // gives up automatic protection against
                            // reordering words relative to each other if you
                            // drag one very far past its neighbors.
                            //
                            // The first/last word are further special-cased:
                            // their line-window bound (`line.start` /
                            // `line.sing_end`) is exactly where their own
                            // *default* position already sits, so without
                            // widening it they'd have the same zero-slack
                            // problem all over again at that one edge - so
                            // let them push past it into the line's
                            // *display* window (up to the previous line's
                            // end, or this line's own `end`), and the
                            // `dragged()` handler below carries the line's
                            // own `start`/`sing_end_override` along with
                            // them when they actually do.
                            let min_start = if w == 0 {
                                prev_line_sing_end.unwrap_or(0.0)
                            } else {
                                line.start
                            };
                            let max_sing_end = if w == last_word_idx && is_backing_lane {
                                // A backing line's end has no neighbor to
                                // stop at - its last word can stretch freely.
                                f64::INFINITY
                            } else if w == last_word_idx {
                                line.end
                            } else {
                                line.sing_end
                            };
                            let bounds = timeline::DragBounds {
                                min_start,
                                max_sing_end,
                            };
                            self.timeline_drag = Some(TimelineDrag {
                                line_idx: orig_idx,
                                target,
                                session: timeline::DragSession::start(
                                    mode,
                                    word.highlight_at,
                                    word.held_until,
                                    bounds,
                                    press_pos.x,
                                ),
                            });
                        }
                    }

                    if bubble_response.dragged() {
                        if let Some(pos) = bubble_response.interact_pointer_pos() {
                            let resolved = self
                                .timeline_drag
                                .as_ref()
                                .filter(|d| d.line_idx == orig_idx && d.target == target)
                                .map(|d| d.session.resolve(pos.x, self.timeline_view.px_per_sec));
                            if let Some((new_start, new_end)) = resolved {
                                if let Some(l) = self.lines.get_mut(orig_idx) {
                                    if let Some(s) = new_start {
                                        if let Some(slot) = l.word_overrides.get_mut(w) {
                                            *slot = Some(s);
                                        }
                                        // The first word's start always tracks
                                        // the line's own start, in both
                                        // directions - not just when pushed
                                        // earlier - so shrinking it back in
                                        // pulls the line bubble back with it
                                        // too, instead of leaving the line
                                        // stuck wherever it was last pushed
                                        // out to. Already safe unconditionally:
                                        // `bounds.min_start` above caps this
                                        // drag at the previous line's own
                                        // sing_end, the same neighbor limit
                                        // `check_start_change` enforces
                                        // elsewhere.
                                        if w == 0 {
                                            l.start = Some(s);
                                        }
                                    }
                                    if let Some(e) = new_end {
                                        if let Some(slot) = l.word_end_overrides.get_mut(w) {
                                            *slot = Some(e);
                                        }
                                        // Symmetrically, the last word's end
                                        // always tracks the line's own
                                        // sing_end - see the matching comment
                                        // above (bounds.max_sing_end caps this
                                        // at the next line's start).
                                        if w == last_word_idx {
                                            l.sing_end_override = Some(e);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if bubble_response.drag_stopped()
                        && self.timeline_drag.as_ref().map(|d| (d.line_idx, d.target))
                            == Some((orig_idx, target))
                    {
                        if let Some(mode) = self.timeline_drag.as_ref().map(|d| d.session.mode) {
                            let seek_near = match mode {
                                timeline::DragMode::RightEdge => self.lines[orig_idx]
                                    .word_end_overrides
                                    .get(w)
                                    .copied()
                                    .flatten()
                                    .unwrap_or(word.held_until),
                                timeline::DragMode::Body | timeline::DragMode::LeftEdge => self
                                    .lines[orig_idx]
                                    .word_overrides
                                    .get(w)
                                    .copied()
                                    .flatten()
                                    .unwrap_or(word.highlight_at),
                            };
                            self.audition_seek(seek_near);
                        }
                        self.timeline_drag = None;
                    }
                }
            }
        }

        // Active loop region, shaded behind the playhead.
        if let Some((from, to)) = self.loop_region() {
            let x0 = self
                .timeline_view
                .time_to_x(rect.left(), from)
                .max(rect.left());
            let x1 = self
                .timeline_view
                .time_to_x(rect.left(), to)
                .min(rect.right());
            if x1 > x0 {
                let band = egui::Rect::from_x_y_ranges(x0..=x1, rect.top()..=rect.bottom());
                painter.rect_filled(
                    band,
                    0.0,
                    egui::Color32::from_rgba_unmultiplied(90, 160, 255, 28),
                );
                for x in [x0, x1] {
                    painter.line_segment(
                        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                        egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(90, 160, 255)),
                    );
                }
            }
        }

        // Playhead - drawn last so it's never hidden behind a bubble.
        let playing = self.audio.as_ref().map(|a| a.is_playing()).unwrap_or(false);
        if let Some(audio) = &self.audio {
            let x = self.timeline_view.time_to_x(rect.left(), audio.position());
            if (rect.left()..=rect.right()).contains(&x) {
                painter.line_segment(
                    [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                    egui::Stroke::new(2.0_f32, egui::Color32::WHITE),
                );
            }
        }

        // Clicking or dragging empty space (checked after the bubble loops
        // so a same-frame bubble drag/click already populated
        // `timeline_drag` or was consumed there and takes priority) seeks
        // playback there - a click jumps once, a drag scrubs continuously,
        // matching the seek bar up top but right on the timeline itself.
        if self.timeline_drag.is_none() && (response.clicked() || response.dragged()) {
            if let Some(pos) = response.interact_pointer_pos() {
                let t = self.timeline_view.x_to_time(rect.left(), pos.x).max(0.0);
                if let Some(audio) = &mut self.audio {
                    let _ = audio.seek(t);
                }
            }
        }

        // Shift+scroll pans instead of zooming - the primary way to look at
        // a different part of a long song without playing/scrubbing there,
        // now that a plain drag scrubs instead of panning.
        if response.hovered() && ui.input(|i| i.modifiers.shift) {
            let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll_y != 0.0 {
                self.timeline_view.pan_by_pixels(scroll_y);
            }
        }

        // While playing (and not mid-scrub/drag), keep the playhead in view
        // instead of leaving it to run off the edge of a zoomed-in window.
        if playing && self.timeline_drag.is_none() && !response.dragged() {
            let t = self.audio.as_ref().map(|a| a.position()).unwrap_or(0.0);
            let visible_secs = (rect.width() / self.timeline_view.px_per_sec) as f64;
            let margin = visible_secs * 0.1;
            if t < self.timeline_view.scroll_secs + margin
                || t > self.timeline_view.scroll_secs + visible_secs - margin
            {
                self.timeline_view.scroll_secs = (t - visible_secs * 0.2).max(0.0);
            }
        }
    }

    /// Small brighter strips at a bubble's left/right edges hinting that
    /// they can be grabbed to resize - scaled down for narrow bubbles
    /// exactly like [`timeline::classify_drag`] does, so the strips never
    /// claim more of the bubble than actually responds as an edge.
    fn draw_edge_grab_strips(painter: &egui::Painter, bubble_rect: egui::Rect) {
        let edge_w = timeline::EDGE_GRAB_PX.min(bubble_rect.width() / 2.0);
        if edge_w <= 0.5 {
            return;
        }
        let edge_color = egui::Color32::from_white_alpha(70);
        let left_edge =
            egui::Rect::from_min_size(bubble_rect.min, egui::vec2(edge_w, bubble_rect.height()));
        let right_edge = egui::Rect::from_min_size(
            egui::pos2(bubble_rect.right() - edge_w, bubble_rect.top()),
            egui::vec2(edge_w, bubble_rect.height()),
        );
        painter.rect_filled(left_edge, 0.0, edge_color);
        painter.rect_filled(right_edge, 0.0, edge_color);
    }
}

/// How often the crash-recovery autosave is rewritten while there's
/// something worth recovering - frequent enough that a crash doesn't lose
/// much, infrequent enough not to matter for disk I/O.
const AUTOSAVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

impl eframe::App for KaraokeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // An autosave found at startup means the last run didn't shut down
        // cleanly - ask before drawing anything else, rather than silently
        // discarding or silently resuming into it.
        if self.pending_recovery.is_some() {
            self.draw_recovery_prompt(ctx);
            return;
        }

        // Intercept a window-close request while there are unsaved
        // changes, so an accidental close (or a deliberate one before
        // remembering to save) gets a chance to reconsider - the
        // crash-recovery autosave already protects the *data* either way,
        // but catching it before the app actually exits is worth doing
        // too, not just after the fact.
        if ctx.input(|i| i.viewport().close_requested())
            && !self.quit_confirmed
            && self.has_unsaved_changes()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.show_quit_confirm = true;
        }
        if self.show_quit_confirm {
            self.draw_quit_confirm_prompt(ctx);
            return;
        }
        if self.pending_unsaved_action.is_some() {
            self.draw_unsaved_action_confirm_prompt(ctx);
            return;
        }

        self.handle_dropped_files(ctx);
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            self.draw_drop_hint(ctx);
        }

        // Periodic crash-recovery autosave, while there's something worth
        // recovering. Cheap (a small JSON write), so it's fine to do
        // synchronously on the UI thread rather than a background one.
        let has_recoverable_content =
            !self.lines.is_empty() || self.audio.as_ref().map(|a| a.is_loaded()).unwrap_or(false);
        if has_recoverable_content {
            let now = std::time::Instant::now();
            let due = self
                .last_autosave
                .map(|t| now.duration_since(t) >= AUTOSAVE_INTERVAL)
                .unwrap_or(true);
            if due {
                self.last_autosave = Some(now);
                let snapshot = self.to_project_file();
                let _ = project::write_autosave(APP_ID, &snapshot);
            }
        }

        let playing = self.audio.as_ref().map(|a| a.is_playing()).unwrap_or(false);
        if playing {
            ctx.request_repaint();
            if self
                .audio
                .as_ref()
                .map(|a| a.finished_naturally())
                .unwrap_or(false)
            {
                self.audio.as_mut().unwrap().stop();
            }
        }
        if self.poll_combined_export() {
            ctx.request_repaint();
        }
        if self.poll_waveform_job() {
            ctx.request_repaint();
        }
        if self.poll_align_job() {
            ctx.request_repaint();
        }
        if self.poll_font_list_job() {
            ctx.request_repaint();
        }
        self.ensure_font_loaded();
        self.ensure_egui_font_installed(ctx);
        self.apply_loop();
        self.auto_follow_word_tap_line();

        self.handle_keyboard_shortcuts(ctx);
        self.sync_ui_style(ctx);

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("Abyssal CDG Creator");
            });
            ui.add_space(4.0);
            self.draw_step_bar(ui);
            ui.add_space(4.0);

            if let Some(err) = &self.audio_error {
                ui.colored_label(egui::Color32::RED, err);
            }

            ui.horizontal(|ui| {
                if ui.button("🆕 New Project").clicked() {
                    self.request_new_project();
                }
                if ui.button("💾 Save Project").clicked() {
                    self.save_project();
                }
                if ui.button("Save Project As…").clicked() {
                    self.save_project_as_dialog();
                }
                if ui.button("📂 Load Project…").clicked() {
                    self.load_project_dialog();
                }
                self.draw_recent_files_menu(ui);
                ui.separator();
                ui.add_enabled_ui(self.can_undo(), |ui| {
                    if ui.button("↶ Undo").clicked() {
                        self.undo();
                    }
                });
                ui.add_enabled_ui(self.can_redo(), |ui| {
                    if ui.button("↷ Redo").clicked() {
                        self.redo();
                    }
                });
                ui.separator();
                if ui.button("📖 Codex").clicked() {
                    self.codex_open = true;
                }
                match &self.current_project_path {
                    Some(path) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        ui.label(format!("({name})"));
                    }
                    None => {
                        ui.label(egui::RichText::new("(unsaved project)").weak());
                    }
                }
            });
            help_text(
                ui,
                self.prefs.show_tips,
                "Ctrl+S save · Ctrl+Z undo · Ctrl+Shift+Z redo.",
                format!(
                    "Saves everything - lyrics, timing, colors, the audio file's path - \
                         to a .{} project file, so you can pick up where you left off. \
                         Ctrl+S saves; Ctrl+Shift+S always asks for a location. Ctrl+Z undoes \
                         a tap/drag/nudge/edit; Ctrl+Shift+Z (or Ctrl+Y) redoes it.",
                    project::FILE_EXTENSION
                ),
            );

            ui.horizontal(|ui| {
                if ui.button("Load Audio…").clicked() {
                    self.load_audio_dialog();
                }
                if let Some(audio) = &self.audio {
                    if let Some(name) = audio.file_name() {
                        ui.label(name);
                        if let Some(d) = audio.duration() {
                            ui.label(format!("({})", format_time(d)));
                        } else {
                            ui.label("(unknown length)");
                        }
                    } else {
                        ui.label("No audio loaded");
                    }
                }
            });

            // Transport row: ⏮, one Play/Pause toggle, ⏪ seek bar ⏩, and a
            // time readout you can click to type a time to jump to.
            ui.horizontal(|ui| {
                let known_duration = self.audio.as_ref().and_then(|a| a.duration());
                let has_audio = self.audio.as_ref().is_some_and(|a| a.is_loaded());
                ui.add_enabled_ui(has_audio, |ui| {
                    if ui
                        .button("⏮")
                        .on_hover_text("Back to the start (keeps playing if it was).")
                        .clicked()
                    {
                        if let Some(a) = &mut self.audio {
                            if let Err(e) = a.seek(0.0) {
                                self.status = format!("Seek error: {e}");
                            }
                        }
                    }
                    let playing = self.audio.as_ref().is_some_and(|a| a.is_playing());
                    let (label, hover) = if playing {
                        (
                            "⏸ Pause",
                            "Pause playback. Space also pauses once every line is timed.",
                        )
                    } else {
                        ("▶ Play", "Play (or resume) from the current position.")
                    };
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(label)
                                    .color(egui::Color32::WHITE)
                                    .strong(),
                            )
                            .fill(ACCENT)
                            .min_size(egui::vec2(84.0, 0.0)),
                        )
                        .on_hover_text(hover)
                        .clicked()
                    {
                        self.toggle_play_pause();
                    }
                    let live_pos = self.audio.as_ref().map(|a| a.position()).unwrap_or(0.0);
                    // If we don't know the song's length (rare - some files
                    // don't report a frame count), fall back to a range that
                    // always extends a bit past the current position rather
                    // than a useless near-zero range.
                    let duration = known_duration.unwrap_or((live_pos + 30.0).max(60.0));

                    if ui.button("⏪ 5s").clicked() {
                        let target = (live_pos - 5.0).max(0.0);
                        if let Some(a) = &mut self.audio {
                            let _ = a.seek(target);
                        }
                    }

                    // Show the drag-in-progress value (if any) instead of the
                    // live position, so the handle doesn't jump around under
                    // the user's cursor while they're scrubbing.
                    let mut display_pos = self.seek_drag_value.unwrap_or(live_pos);
                    let resp = ui.add(
                        egui::Slider::new(&mut display_pos, 0.0..=duration)
                            .custom_formatter(|v, _| format_time(v))
                            .trailing_fill(true),
                    );
                    if resp.dragged() {
                        self.seek_drag_value = Some(display_pos);
                    } else if let Some(target) = self.seek_drag_value.take() {
                        // Was dragging last frame, isn't anymore - commit the seek.
                        if let Some(a) = &mut self.audio {
                            if let Err(e) = a.seek(target) {
                                self.status = format!("Seek error: {e}");
                            }
                        }
                    }

                    if ui.button("5s ⏩").clicked() {
                        let target = live_pos + 5.0;
                        if let Some(a) = &mut self.audio {
                            let _ = a.seek(target);
                        }
                    }

                    self.draw_time_readout(ui, known_duration);
                });
                if known_duration.is_none() && has_audio {
                    ui.label("(exact song length unknown - seek range is approximate)");
                }
            });

            // Speed and loop share one row.
            ui.horizontal_wrapped(|ui| {
                let has_audio = self.audio.as_ref().is_some_and(|a| a.is_loaded());
                ui.add_enabled_ui(has_audio, |ui| {
                    ui.label("Speed:");
                    let current = self.audio.as_ref().map(|a| a.speed()).unwrap_or(1.0);
                    for (speed, label) in [(0.5f32, "0.5×"), (0.75, "0.75×"), (1.0, "1×")] {
                        let selected = (current - speed).abs() < 0.001;
                        if ui
                            .selectable_label(selected, label)
                            .on_hover_text(
                                "Slow the song down for tapping fast lines or fine-tuning \
                                 words. Taps still record the real song time. Pitch drops \
                                 along with speed - this only affects playback here, never \
                                 the exported files.",
                            )
                            .clicked()
                        {
                            if let Some(a) = &mut self.audio {
                                a.set_speed(speed);
                            }
                        }
                    }
                    if current < 0.999 {
                        ui.label(
                            egui::RichText::new("slowed down - exports are always full speed")
                                .small()
                                .weak(),
                        );
                    }
                    ui.separator();
                    ui.label("Loop:");
                    let line_loop = match self.loop_mode {
                        Some(LoopMode::Line(i)) => Some(i),
                        _ => None,
                    };
                    let label = match line_loop {
                        Some(i) => format!("🔁 Line {}  [L]", i + 1),
                        None => "🔁 Loop line  [L]".to_string(),
                    };
                    if ui
                        .selectable_label(line_loop.is_some(), label)
                        .on_hover_text(
                            "Repeat the line open in the fine-tune panel (or the line being \
                             tapped), with a second of lead-in, so you can click its words \
                             again and again until they land. Follows the line's timing as \
                             you change it. Click again (or press L) to stop.",
                        )
                        .clicked()
                    {
                        self.toggle_loop_line(None);
                    }
                    ui.separator();
                    let ab = match self.loop_mode {
                        Some(LoopMode::Ab { a, b }) => Some((a, b)),
                        _ => None,
                    };
                    if ui
                        .selectable_label(ab.is_some(), "A")
                        .on_hover_text("Mark the current position as the loop start.")
                        .clicked()
                    {
                        self.set_loop_a();
                    }
                    if ui
                        .selectable_label(ab.is_some_and(|(_, b)| b.is_some()), "B")
                        .on_hover_text(
                            "Mark the current position as the loop end and start looping \
                             between A and B.",
                        )
                        .clicked()
                    {
                        self.set_loop_b();
                    }
                    if self.loop_mode.is_some() {
                        if ui
                            .small_button("✖")
                            .on_hover_text("Stop looping.")
                            .clicked()
                        {
                            self.loop_mode = None;
                            self.status = "Loop off.".to_string();
                        }
                        if let Some((from, to)) = self.loop_region() {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} - {}",
                                    lyrics::format_timecode(from),
                                    lyrics::format_timecode(to)
                                ))
                                .small()
                                .weak(),
                            );
                        }
                    }
                });
            });

            help_text(
                ui,
                self.prefs.show_tips,
                "Drag to jump · click the time to type one · ←/→ keys skip.",
                "Drag the bar (or use ⏪/⏩, or the left/right arrow keys) to jump anywhere in \
                 the song - handy for redoing a line without replaying from the start. Or \
                 click the time readout, type a time (1:23.5, or seconds) and press Enter.",
            );

            ui.horizontal(|ui| {
                ui.label("Song title:");
                ui.add(egui::TextEdit::singleline(&mut self.title).desired_width(220.0));
                ui.label("Artist:");
                ui.add(egui::TextEdit::singleline(&mut self.artist).desired_width(220.0));
            });
            help_text(
                ui,
                self.prefs.show_tips,
                "Tip: \"Artist - Song.mp3\" fills these in.",
                "Tip: name imported audio files \"Artist - Song Name.ext\" and these \
                     fields fill in automatically when they're both still blank.",
            );

            ui.horizontal(|ui| {
                ui.checkbox(&mut self.outro.enabled, "Outro card")
                    .on_hover_text(
                        "A closing card after the last line, until the song ends - in the \
                         video, the .cdg, the subtitles and the live preview. Needs at \
                         least 2 seconds of song left after the last line.",
                    );
                ui.add_enabled_ui(self.outro.enabled, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.outro.text)
                            .desired_rows(2)
                            .desired_width(240.0)
                            .hint_text("Thanks for singing!"),
                    )
                    .on_hover_text(format!(
                        "Up to {} lines - the first is shown largest.",
                        lyrics::MAX_OUTRO_LINES
                    ));
                    ui.checkbox(&mut self.outro.show_title, "+ title & artist");
                });
            });
            if self.outro.enabled {
                let (timed, total) = self.resolved();
                let backing = self.resolved_backing();
                let note = match export::outro_window(&timed, &backing, total) {
                    Some((start, end)) => format!(
                        "Shown {} - {} (from {:.1}s after the last line).",
                        lyrics::format_timecode(start),
                        lyrics::format_timecode(end),
                        export::OUTRO_GAP_SECS
                    ),
                    None if timed.is_empty() => {
                        "Shown after the last line, once lines are timed.".to_string()
                    }
                    None => format!(
                        "Not shown: the song ends less than {:.1}s after the last line.",
                        export::OUTRO_GAP_SECS + export::MIN_OUTRO_SECS
                    ),
                };
                ui.label(egui::RichText::new(note).small().weak());
            }

            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut self.prefs.show_tips, "💡 Tips")
                    .on_hover_text(
                        "Show the full explanations under each control. Turn off once you \
                         know your way around - each becomes a one-line summary, with the \
                         full text on its ⓘ.",
                    )
                    .changed()
                {
                    prefs::save(APP_ID, &self.prefs);
                }
                if ui
                    .button("⌨ Shortcuts")
                    .on_hover_text("Every keyboard shortcut (F1 or ?).")
                    .clicked()
                {
                    self.show_shortcuts = !self.show_shortcuts;
                }
                ui.separator();
                ui.label("UI size:");
                let zoom = ui.ctx().zoom_factor();
                if ui
                    .add_enabled(
                        zoom > prefs::MIN_UI_ZOOM + 0.001,
                        egui::Button::new("−").small(),
                    )
                    .on_hover_text("Smaller (Ctrl+-)")
                    .clicked()
                {
                    egui::gui_zoom::zoom_out(ui.ctx());
                }
                if ui
                    .small_button(format!("{:.0}%", zoom * 100.0))
                    .on_hover_text("Back to 100% (Ctrl+0)")
                    .clicked()
                {
                    ui.ctx().set_zoom_factor(1.0);
                }
                if ui
                    .add_enabled(
                        zoom < prefs::MAX_UI_ZOOM - 0.001,
                        egui::Button::new("+").small(),
                    )
                    .on_hover_text("Bigger (Ctrl+=)")
                    .clicked()
                {
                    egui::gui_zoom::zoom_in(ui.ctx());
                }
            });
            ui.checkbox(&mut self.auto_follow_words, "Auto-follow fine-tune panel")
                .on_hover_text(
                    "While the \"Fine-tune words\" panel is open and the song is playing, it \
                     normally jumps to whichever line is currently playing. Turn this off to \
                     keep it on one line until you pick another yourself - handy for catching \
                     a line's very first or last word, which auto-follow can otherwise snatch \
                     the panel away from (or onto) right as it needs a click.",
                );
        });

        self.draw_export_dialog(ctx);
        self.draw_codex(ctx);
        self.draw_shortcuts_window(ctx);
        self.draw_copy_timing_dialog(ctx);
        self.track_status();
        self.draw_toast(ctx);
        self.draw_disclaimer_modal(ctx);

        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| {
            ui.add_space(4.0);
            let busy = self.project_busy();
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if ui
                        .add_sized(
                            [120.0, 28.0],
                            egui::Button::new(
                                egui::RichText::new("Export…")
                                    .color(egui::Color32::WHITE)
                                    .strong(),
                            )
                            .fill(ACCENT),
                        )
                        .clicked()
                    {
                        self.open_export_dialog();
                    }
                    if ui.button("Reset all timing").clicked() {
                        self.reset_timing();
                    }
                    if ui.button("🪄 Auto-align words").clicked() {
                        self.start_word_alignment();
                    }
                    ui.label("Language:");
                    egui::ComboBox::from_id_source("align_language")
                        .selected_text(format!(
                            "{} ({})",
                            self.align_language.label(),
                            self.align_language.code()
                        ))
                        .show_ui(ui, |ui| {
                            for lang in align::AlignLanguage::ALL {
                                ui.selectable_value(
                                    &mut self.align_language,
                                    lang,
                                    format!("{} ({})", lang.label(), lang.code()),
                                );
                            }
                        });
                    ui.label("Vocal model:").on_hover_text(
                        "Which model isolates vocals before alignment - doesn't affect \
                         \"Instrumental audio\"/\"Vocals audio\" export or video's \"Remove \
                         vocals\", which always use Inst HQ 3.",
                    );
                    egui::ComboBox::from_id_source("align_vocal_model")
                        .selected_text(self.align_vocal_model.label())
                        .show_ui(ui, |ui| {
                            for model in mdx::MdxModel::ALL {
                                ui.selectable_value(
                                    &mut self.align_vocal_model,
                                    model,
                                    model.label(),
                                );
                            }
                        });
                });
                if busy {
                    ui.spinner();
                }
            });
            help_text(
                ui,
                self.prefs.show_tips,
                "Auto-align fills in word timing from the audio - always review it.",
                "Auto-align isolates vocals first, then runs a real speech-recognition \
                     model (once per already-timed line) against just the singing to fill in \
                     every word's timing automatically, replacing any existing word timing \
                     (estimated or manually tapped) for lines it successfully aligns - the song \
                     you hear back and tap timestamps against is unaffected either way. Nothing \
                     to install separately - the selected language's model downloads \
                     automatically the first time you use it (a one-time, roughly 1.2GB \
                     download, then cached). Best-effort, not perfect: it's a speech model, \
                     not one trained for singing, so always play the result back and fine-tune \
                     anything that's off before exporting - treat it as a fast first pass, not \
                     a substitute for a final review. Undo (Ctrl+Z) if a result doesn't help.",
            );
            if let Some(handle) = &self.combined_export {
                let frac = handle.progress.load(Ordering::Relaxed) as f32 / 1000.0;
                ui.add(egui::ProgressBar::new(frac).show_percentage());
            }
            if let Some(handle) = &self.align_job {
                let frac = handle.progress.load(Ordering::Relaxed) as f32 / 1000.0;
                ui.add(egui::ProgressBar::new(frac).show_percentage());
            }
            if !self.status.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    let kind = classify_status(&self.status);
                    ui.label(
                        egui::RichText::new(format!("{}{}", kind.icon(), self.status))
                            .color(kind.color(ui.visuals())),
                    );
                    self.open_folder_button(ui);
                });
            }
            ui.add_space(4.0);
        });

        egui::TopBottomPanel::bottom("timeline_panel")
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                self.draw_timeline(ui);
                ui.add_space(4.0);
            });

        egui::SidePanel::left("lyrics_panel")
            .resizable(true)
            .default_width(360.0)
            .width_range(280.0..=560.0)
            .show(ctx, |ui| {
                ui.label("1. Paste the song lyrics (one line per line of text):");
                egui::ScrollArea::vertical()
                    .id_source("lyrics_scroll")
                    .max_height(300.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let len_before = self.lyrics_raw.chars().count();
                        let output = egui::TextEdit::multiline(&mut self.lyrics_raw)
                            .desired_rows(14)
                            .desired_width(f32::INFINITY)
                            .show(ui);
                        if output.response.changed() {
                            if let Some(cursor) = output.cursor_range {
                                let delta =
                                    self.lyrics_raw.chars().count() as isize - len_before as isize;
                                self.lyrics_edited_span = Some(track_lyrics_edit(
                                    self.lyrics_edited_span.take(),
                                    cursor.primary.ccursor.index,
                                    delta,
                                ));
                            }
                        }
                    });
                if ui.button("Parse lyrics").clicked() {
                    self.parse_lyrics();
                }
                if ui.button("Load lyrics file…").clicked() {
                    self.load_lyrics_file_dialog();
                }
                help_text(
                    ui,
                    self.prefs.show_tips,
                    "LRC, UltraStar and KOK files are supported.",
                    "Supports LRC (.lrc), UltraStar (.txt), and KOK - auto-detected.",
                );
                help_text(
                    ui,
                    self.prefs.show_tips,
                    "Tip: beau|ti|ful times each syllable.",
                    "Tip: split a long held word into syllables with | (e.g. beau|ti|ful) \
                         to time each syllable on its own - it still shows as one word.",
                );
                ui.add_space(8.0);
                ui.separator();
                ui.label("2. Tap along:");
                help_text(
                    ui,
                    self.prefs.show_tips,
                    "Space: tap each line's start, then its end · Shift+Space: end + start \
                     next · M/F/D/S/B: voice.",
                    "Press Play, then press Space (or click below) the instant each line \
                     starts being sung, and again the instant it ends - two taps per line. \
                     Lines fill in top to bottom automatically - missed one? Drag the seek \
                     bar back a few seconds and try again. (Skipping the end-tap is fine too - \
                     it'll just fall back to an automatic estimate until you set it, here or \
                     in the table below.) Line runs straight into the next one? Shift+Space \
                     ends it and starts the next in one tap. Press M/F/D/S/B to set that \
                     line's voice (Male/Female/Duet/Screaming/Backing) without leaving the \
                     keyboard.",
                );
                ui.horizontal(|ui| {
                    ui.label("Tap latency:");
                    let resp = ui
                        .add(
                            egui::DragValue::new(&mut self.prefs.tap_latency_ms)
                                .range(0.0..=prefs::MAX_TAP_LATENCY_MS)
                                .speed(1.0)
                                .fixed_decimals(0)
                                .suffix(" ms"),
                        )
                        .on_hover_text(
                            "Subtracted from every tap (tap-along, re-tap, backing lines, and \
                             word/line-end taps in the fine-tune panel), to cancel out how \
                             late you tap after hearing a line start - typically 100-200 ms, \
                             more with Bluetooth headphones. If your taps consistently land \
                             a bit late on the timeline, raise this. Saved on this computer, \
                             not in the project, and only affects new taps.",
                        );
                    // Save when an edit finishes, not on every frame of a drag.
                    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                        prefs::save(APP_ID, &self.prefs);
                    }
                });
                let can_tap = self.audio.as_ref().map(|a| a.is_playing()).unwrap_or(false)
                    && self.next_untimed < self.lines.len();
                ui.add_enabled_ui(can_tap, |ui| {
                    let label = if self.next_untimed < self.lines.len() {
                        let phase = match self.tap_phase {
                            TapPhase::Start => "start",
                            TapPhase::End => "end",
                        };
                        format!(
                            "⏱ Tap {} of line ({}/{})  [Space]",
                            phase,
                            self.next_untimed + 1,
                            self.lines.len()
                        )
                    } else {
                        "⏱ Tap next line".to_string()
                    };
                    if ui
                        .add_sized(
                            [ui.available_width(), 48.0],
                            egui::Button::new(
                                egui::RichText::new(label)
                                    .color(egui::Color32::WHITE)
                                    .strong(),
                            )
                            .fill(ACCENT),
                        )
                        .clicked()
                    {
                        self.tap_next();
                    }
                    // Back-to-back lines: one press ends this line and starts
                    // the next at the same instant (see
                    // `tap_end_and_start_next`). Only offered at an end tap
                    // with another line still to go.
                    let has_next_main = self.lines[self.next_untimed.min(self.lines.len())..]
                        .iter()
                        .skip(1)
                        .any(|l| !l.is_backing);
                    if self.tap_phase == TapPhase::End
                        && has_next_main
                        && ui
                            .add_sized(
                                [ui.available_width(), 24.0],
                                egui::Button::new(
                                    "⏭ End this line + start the next  [Shift+Space]",
                                ),
                            )
                            .on_hover_text(
                                "For lines sung straight into each other with no gap: sets \
                                 this line's end and the next line's start at the same \
                                 instant, in one tap.",
                            )
                            .clicked()
                    {
                        self.tap_end_and_start_next();
                    }
                });
            });

        // Max width tracks the window so the panel can be dragged wide for a
        // bigger preview, while always leaving room for the timing table.
        let preview_panel_max = (ctx.screen_rect().width() * 0.65).max(560.0);
        egui::SidePanel::right("colors_preview_panel")
            .resizable(true)
            .default_width(340.0)
            .width_range(260.0..=preview_panel_max)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_source("right_panel_scroll")
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("4. Preview (matches the exported file):");
                            let label = if self.preview_popped_out {
                                "Bring back"
                            } else {
                                "Pop out"
                            };
                            if ui
                                .button(label)
                                .on_hover_text(
                                    "Show the preview in its own window you can resize or \
                                     maximize - keyboard shortcuts keep working there.",
                                )
                                .clicked()
                            {
                                self.preview_popped_out = !self.preview_popped_out;
                            }
                        });
                        if self.preview_popped_out {
                            ui.label(
                                egui::RichText::new(
                                    "Showing in its own window - close it or click \
                                     \"Bring back\" to return it here.",
                                )
                                .weak(),
                            );
                        } else {
                            self.draw_preview(ui, None);
                        }
                        ui.add_space(10.0);
                        ui.separator();
                        let color_issues =
                            color_warnings(&self.export_colors(), &self.voices_in_use());
                        let header = if color_issues.is_empty() {
                            egui::RichText::new("Colors")
                        } else {
                            egui::RichText::new(format!("Colors  ⚠ {}", color_issues.len()))
                                .color(egui::Color32::from_rgb(230, 160, 40))
                        };
                        egui::CollapsingHeader::new(header)
                            .id_source("colors_section")
                            .default_open(false)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label("Preset:");
                                    egui::ComboBox::from_id_source("color_preset")
                                        .selected_text(self.color_preset.label())
                                        .show_ui(ui, |ui| {
                                            for preset in ALL_COLOR_PRESETS {
                                                if ui
                                                    .selectable_label(
                                                        self.color_preset == preset,
                                                        preset.label(),
                                                    )
                                                    .clicked()
                                                {
                                                    self.apply_color_preset(preset);
                                                }
                                            }
                                        });
                                });
                                help_text(
                                    ui,
                                    self.prefs.show_tips,
                                    "",
                                    "Pick a look, or fine-tune individual colors below.",
                                );
                                for issue in &color_issues {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(230, 160, 40),
                                        egui::RichText::new(format!("⚠ {issue}")).small(),
                                    );
                                }
                                ui.add_space(6.0);
                                egui::Grid::new("colors_grid")
                                    .num_columns(2)
                                    .spacing([8.0, 4.0])
                                    .show(ui, |ui| {
                                        ui.label("Background");
                                        ui.color_edit_button_srgba(&mut self.color_bg);
                                        ui.end_row();

                                        ui.label("Default - upcoming").on_hover_text(
                                            "Used for any line whose singer hasn't been \
                                                 manually set. Defaults to the same look as \
                                                 Male, but can be set independently - e.g. to \
                                                 match a background image/video's palette.",
                                        );
                                        ui.color_edit_button_srgba(&mut self.color_default_unsung);
                                        ui.end_row();
                                        ui.label("Default - sung");
                                        ui.color_edit_button_srgba(
                                            &mut self.color_default_highlight,
                                        );
                                        ui.end_row();

                                        ui.label("Male - upcoming");
                                        ui.color_edit_button_srgba(&mut self.color_male_unsung);
                                        ui.end_row();
                                        ui.label("Male - sung");
                                        ui.color_edit_button_srgba(&mut self.color_male_highlight);
                                        ui.end_row();

                                        ui.label("Female - upcoming");
                                        ui.color_edit_button_srgba(&mut self.color_female_unsung);
                                        ui.end_row();
                                        ui.label("Female - sung");
                                        ui.color_edit_button_srgba(
                                            &mut self.color_female_highlight,
                                        );
                                        ui.end_row();

                                        ui.label("Duet - upcoming");
                                        ui.color_edit_button_srgba(&mut self.color_duet_unsung);
                                        ui.end_row();
                                        ui.label("Duet - sung");
                                        ui.color_edit_button_srgba(&mut self.color_duet_highlight);
                                        ui.end_row();

                                        ui.label("Screaming - upcoming");
                                        ui.color_edit_button_srgba(
                                            &mut self.color_screaming_unsung,
                                        );
                                        ui.end_row();
                                        ui.label("Screaming - sung");
                                        ui.color_edit_button_srgba(
                                            &mut self.color_screaming_highlight,
                                        );
                                        ui.end_row();

                                        ui.label("Backing - upcoming");
                                        ui.color_edit_button_srgba(&mut self.color_backing_unsung);
                                        ui.end_row();
                                        ui.label("Backing - sung");
                                        ui.color_edit_button_srgba(
                                            &mut self.color_backing_highlight,
                                        );
                                        ui.end_row();

                                        ui.label("Next-line preview");
                                        ui.color_edit_button_srgba(&mut self.color_preview);
                                        ui.end_row();

                                        ui.label("Title card - title");
                                        ui.color_edit_button_srgba(&mut self.color_title);
                                        ui.end_row();
                                        ui.label("Title card - artist");
                                        ui.color_edit_button_srgba(&mut self.color_artist);
                                        ui.end_row();
                                    });
                            });
                        egui::CollapsingHeader::new("Background")
                            .default_open(false)
                            .show(ui, |ui| {
                                help_text(
                                    ui,
                                    self.prefs.show_tips,
                                    "An image or video behind the lyrics (video and preview only).",
                                    "Show an image or video behind the lyrics in the video \
                                         export and the preview above, instead of the flat \
                                         Background color. The .cdg export always uses the flat \
                                         color instead - it's a fixed 300x216, 16-color format \
                                         with no room for real images.",
                                );
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    if ui.button("Choose Image/Video…").clicked() {
                                        self.choose_background_dialog();
                                    }
                                    if self.background.is_some() && ui.button("Clear").clicked() {
                                        self.background = None;
                                        self.background_preview = None;
                                        self.status = "Background cleared.".to_string();
                                    }
                                });
                                match &self.background {
                                    Some(bg) => {
                                        let kind = match bg {
                                            video::Background::Image(_) => "Image",
                                            video::Background::Video(_) => "Video",
                                        };
                                        let name = bg
                                            .path()
                                            .file_name()
                                            .map(|n| n.to_string_lossy().to_string())
                                            .unwrap_or_default();
                                        ui.label(format!("{kind}: {name}"));
                                    }
                                    None => {
                                        ui.label(egui::RichText::new("No background set.").weak());
                                    }
                                }
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    ui.label("Fit:");
                                    egui::ComboBox::from_id_source("background_fit")
                                        .selected_text(self.background_fit.label())
                                        .show_ui(ui, |ui| {
                                            for option in [
                                                video::BackgroundFit::Cover,
                                                video::BackgroundFit::Contain,
                                            ] {
                                                if ui
                                                    .selectable_label(
                                                        self.background_fit == option,
                                                        option.label(),
                                                    )
                                                    .clicked()
                                                    && self.background_fit != option
                                                {
                                                    self.background_fit = option;
                                                    // Built for the old fit mode - drop it so
                                                    // the preview regenerates.
                                                    self.background_preview = None;
                                                }
                                            }
                                        });
                                });
                                help_text(
                                    ui,
                                    self.prefs.show_tips,
                                    "Cover fills the frame; Contain shows it all.",
                                    "Cover crops the edges so nothing gets stretched; \
                                         Contain shows the whole image/video with bars added \
                                         if the aspect ratio doesn't match.",
                                );
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    ui.label("Dim:");
                                    ui.add(
                                        egui::Slider::new(&mut self.background_dim, 0.0..=1.0)
                                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                                    );
                                });
                                help_text(
                                    ui,
                                    self.prefs.show_tips,
                                    "Darkens the background so lyrics stay readable.",
                                    "How much of a black scrim to blend over the \
                                         background image/video so the lyrics stay legible on \
                                         top of it.",
                                );
                            });
                        egui::CollapsingHeader::new("Font")
                            .default_open(false)
                            .show(ui, |ui| {
                                help_text(
                                    ui,
                                    self.prefs.show_tips,
                                    "Video and preview only - the .cdg uses its own pixel font.",
                                    "Choose a font for lyric text in the video export and \
                                         the preview above. The .cdg export always uses its own \
                                         fixed built-in font instead - it's a 300x216, 16-color \
                                         tile format with no room for an arbitrary scalable font.",
                                );
                                ui.add_space(4.0);

                                ui.horizontal(|ui| {
                                    ui.label("Current:");
                                    ui.strong(
                                        self.selected_font_family
                                            .as_deref()
                                            .unwrap_or("Default (DejaVu Sans)"),
                                    );
                                });
                                if let Some(err) = &self.font_load_error {
                                    ui.colored_label(egui::Color32::from_rgb(230, 140, 40), err);
                                }

                                ui.horizontal(|ui| {
                                    let label = if self.font_picker_open {
                                        "Close picker"
                                    } else {
                                        "Choose font…"
                                    };
                                    if ui.button(label).clicked() {
                                        self.font_picker_open = !self.font_picker_open;
                                    }
                                    if self.selected_font_family.is_some()
                                        && ui.button("Reset to default").clicked()
                                    {
                                        self.selected_font_family = None;
                                        self.font_bytes = None;
                                        self.font_load_error = None;
                                    }
                                });

                                if self.font_picker_open {
                                    ui.add_space(4.0);
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.font_search)
                                            .hint_text("Search fonts…")
                                            .desired_width(f32::INFINITY),
                                    );
                                    match &self.font_list {
                                        None => {
                                            ui.label(egui::RichText::new("Loading fonts…").weak());
                                        }
                                        Some(list) => {
                                            // Cloned to a plain owned Vec so
                                            // the picker's click handling
                                            // below is free to mutate
                                            // `self` without fighting a
                                            // borrow of `self.font_list`.
                                            let names = list.clone();
                                            let query = self.font_search.to_lowercase();
                                            let filtered: Vec<&String> = names
                                                .iter()
                                                .filter(|n| {
                                                    query.is_empty()
                                                        || n.to_lowercase().contains(&query)
                                                })
                                                .collect();
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} font(s)",
                                                    filtered.len()
                                                ))
                                                .small()
                                                .weak(),
                                            );
                                            let mut pick: Option<String> = None;
                                            egui::ScrollArea::vertical()
                                                .max_height(180.0)
                                                .id_source("font_picker_scroll")
                                                .show(ui, |ui| {
                                                    for name in &filtered {
                                                        let selected =
                                                            self.selected_font_family.as_deref()
                                                                == Some(name.as_str());
                                                        if ui
                                                            .selectable_label(
                                                                selected,
                                                                name.as_str(),
                                                            )
                                                            .clicked()
                                                        {
                                                            pick = Some((*name).clone());
                                                        }
                                                    }
                                                    if filtered.is_empty() {
                                                        ui.label(
                                                            egui::RichText::new(
                                                                "No fonts match your search.",
                                                            )
                                                            .weak(),
                                                        );
                                                    }
                                                });
                                            if let Some(name) = pick {
                                                if self.selected_font_family.as_deref()
                                                    != Some(name.as_str())
                                                {
                                                    self.selected_font_family = Some(name);
                                                    self.font_bytes = None;
                                                    self.font_load_error = None;
                                                }
                                                self.font_picker_open = false;
                                            }
                                        }
                                    }
                                }
                            });
                        egui::CollapsingHeader::new("Timing")
                            .default_open(false)
                            .show(ui, |ui| {
                                help_text(
                                    ui,
                                    self.prefs.show_tips,
                                    "Tunes the automatic estimates.",
                                    "Tunes the word-timing estimate and the \"get ready\" \
                                         countdown's sensitivity - used only where you haven't \
                                         fine-tuned something yourself (a manually tapped word, \
                                         or an auto-aligned line). Saved with the project.",
                                );
                                ui.add_space(4.0);

                                ui.horizontal(|ui| {
                                    ui.label("Seconds per word:");
                                    ui.add(
                                        egui::Slider::new(
                                            &mut self.timing_settings.seconds_per_word,
                                            0.1..=1.5,
                                        )
                                        .custom_formatter(|v, _| format!("{v:.2}s")),
                                    );
                                });
                                help_text(
                                    ui,
                                    self.prefs.show_tips,
                                    "Estimated time per word.",
                                    "How long each word is assumed to take to sing, for the \
                                         word-by-word color wipe - only matters for words you \
                                         haven't fine-tuned by hand.",
                                );
                                ui.add_space(6.0);

                                ui.horizontal(|ui| {
                                    ui.label("Minimum line duration:");
                                    ui.add(
                                        egui::Slider::new(
                                            &mut self.timing_settings.min_sing_duration,
                                            0.3..=5.0,
                                        )
                                        .custom_formatter(|v, _| format!("{v:.2}s")),
                                    );
                                });
                                help_text(
                                    ui,
                                    self.prefs.show_tips,
                                    "Shortest estimated line.",
                                    "A floor under the estimate above, so even a single \
                                         short word still gets a readable amount of time on \
                                         screen.",
                                );
                                ui.add_space(6.0);

                                ui.horizontal(|ui| {
                                    ui.label("Countdown gap threshold:");
                                    ui.add(
                                        egui::Slider::new(
                                            &mut self.timing_settings.countdown_gap_threshold,
                                            1.0..=15.0,
                                        )
                                        .custom_formatter(|v, _| format!("{v:.1}s")),
                                    );
                                });
                                help_text(
                                    ui,
                                    self.prefs.show_tips,
                                    "Gap that triggers the countdown.",
                                    "How long a gap before a line starts (or before the \
                                         first line, after the title card) needs to be before \
                                         the \"get ready\" countdown dots appear.",
                                );
                                ui.add_space(6.0);

                                if ui.button("Reset to defaults").clicked() {
                                    self.timing_settings = lyrics::TimingSettings::default();
                                }
                            });
                    });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("3. Review / fine-tune timestamps and voice:");
            if !self.lines.is_empty() {
                let statuses = line_statuses(&self.lines);
                let count = |k: LineStatus| statuses.iter().filter(|s| **s == k).count();
                let (untimed, estimated, tuned) = (
                    count(LineStatus::Untimed),
                    count(LineStatus::Estimated),
                    count(LineStatus::FineTuned),
                );
                let timed = statuses.len() - untimed;
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::ProgressBar::new(timed as f32 / statuses.len() as f32)
                            .desired_width(120.0)
                            .fill(ACCENT),
                    );
                    let visuals = ui.visuals().clone();
                    let part = |ui: &mut egui::Ui, k: LineStatus, text: String| {
                        ui.label(egui::RichText::new("●").color(k.color(&visuals)));
                        ui.label(text).on_hover_text(k.describe());
                    };
                    part(ui, LineStatus::Untimed, format!("{timed}/{} timed", statuses.len()));
                    if estimated > 0 {
                        part(ui, LineStatus::Estimated, format!("{estimated} estimated"));
                    }
                    part(ui, LineStatus::FineTuned, format!("{tuned} fine-tuned"));
                });
            }

            // Per-word fine-tuning strip - only shown while a line is selected.
            if let Some(i) = self.word_tap_line {
                if i < self.lines.len() {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.strong("Fine-tune words:");
                            ui.label(&self.lines[i].text);
                            let looping_this = self.loop_mode == Some(LoopMode::Line(i));
                            if ui
                                .selectable_label(looping_this, "🔁 Loop this line")
                                .on_hover_text(
                                    "Repeat this line (with a second of lead-in) while you \
                                     click its words. Shortcut: L.",
                                )
                                .clicked()
                            {
                                self.toggle_loop_line(Some(i));
                            }
                            if ui.small_button("Close").clicked() {
                                self.word_tap_line = None;
                            }
                        });
                        help_text(ui, self.prefs.show_tips, "Click each word the instant it's sung.", if self.auto_follow_words {
                            "Play the song and just click each word the instant it's sung - \
                             this panel follows along automatically as the song moves from \
                             line to line, so there's no need to reselect a line yourself. \
                             Click a word again to retime it. If auto-follow keeps snatching \
                             the panel away before you can catch a line's first or last word, \
                             turn off Auto-follow up in the top panel."
                        } else {
                            "Auto-follow (top panel) is off - this panel stays on this line \
                             until you pick another one (click any lyric line below, or a \
                             bubble on the timeline), so a line's first/last word is easier to \
                             catch. Click a word again to retime it."
                        });
                        ui.horizontal(|ui| {
                            ui.label("Tap sets a word's:");
                            ui.selectable_value(
                                &mut self.word_tap_mode,
                                WordTapMode::Start,
                                "Start",
                            );
                            ui.selectable_value(&mut self.word_tap_mode, WordTapMode::End, "End");
                            help_text(
                                ui,
                                self.prefs.show_tips,
                                "End: where a held word stops.",
                                "- by default a word's highlight just runs until the next \
                                 word starts; set an End to make it stop (and freeze) sooner, \
                                 e.g. for a held note followed by a pause.",
                            );
                        });
                        help_text(
                            ui,
                            self.prefs.show_tips,
                            "Green start · blue end · teal both.",
                            "Green = start manually timed, blue = end manually timed, teal = \
                             both; the rest still use the automatic estimate.",
                        );
                        // One button per timing unit: a word marked `beau|ti|ful`
                        // gets a button per syllable, shown with a trailing/
                        // leading dash so they read as one word.
                        let words: Vec<String> = {
                            let units = lyrics::timing_units(&self.lines[i].text);
                            let mut prev_ended_word = true;
                            units
                                .iter()
                                .map(|&(u, ends_word)| {
                                    let mut label = String::new();
                                    if !prev_ended_word {
                                        label.push('-');
                                    }
                                    label.push_str(u);
                                    if !ends_word {
                                        label.push('-');
                                    }
                                    prev_ended_word = ends_word;
                                    label
                                })
                                .collect()
                        };
                        ui.horizontal_wrapped(|ui| {
                            for (w_idx, word) in words.iter().enumerate() {
                                let manual_start =
                                    self.lines[i].word_overrides.get(w_idx).copied().flatten();
                                let manual_end = self.lines[i]
                                    .word_end_overrides
                                    .get(w_idx)
                                    .copied()
                                    .flatten();
                                let text = match (manual_start, manual_end) {
                                    (Some(s), Some(e)) => format!("{word} ({s:.1}-{e:.1}s)"),
                                    (Some(s), None) => format!("{word} ({s:.1}s+)"),
                                    (None, Some(e)) => format!("{word} (+{e:.1}s)"),
                                    (None, None) => word.clone(),
                                };
                                let fill = match (manual_start.is_some(), manual_end.is_some()) {
                                    (true, true) => egui::Color32::from_rgb(30, 90, 90),
                                    (true, false) => egui::Color32::from_rgb(35, 90, 45),
                                    (false, true) => egui::Color32::from_rgb(35, 60, 100),
                                    (false, false) => ui.visuals().widgets.inactive.weak_bg_fill,
                                };
                                let button = egui::Button::new(text).fill(fill);
                                if ui.add(button).clicked() {
                                    match self.word_tap_mode {
                                        WordTapMode::Start => self.tap_word_start(i, w_idx),
                                        WordTapMode::End => self.tap_word_end(i, w_idx),
                                    }
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            let manual_end = self.lines[i].sing_end_override;
                            let text = match manual_end {
                                Some(t) => format!("End of line ({t:.1}s)"),
                                None => "End of line (auto)".to_string(),
                            };
                            let button = egui::Button::new(text).fill(if manual_end.is_some() {
                                egui::Color32::from_rgb(35, 90, 45)
                            } else {
                                ui.visuals().widgets.inactive.weak_bg_fill
                            });
                            if ui.add(button).clicked() {
                                self.tap_line_end(i);
                            }
                            ui.label(
                                "- tap the instant the last word finishes, so its \
                                 held-out highlight ends exactly on time.",
                            );
                        });
                        if ui
                            .small_button("Reset word & end timing for this line")
                            .clicked()
                        {
                            self.reset_word_overrides(i);
                        }
                    });
                    ui.add_space(6.0);
                } else {
                    self.word_tap_line = None;
                }
            }

            // `self.lines` can change out from under a stale selection
            // (re-parsing, undo/redo, loading a different file, ...) -
            // drop anything that no longer points at a real line before
            // the selection toolbar/table below read it this frame.
            self.selected_lines.retain(|&i| i < self.lines.len());

            ui.horizontal(|ui| {
                if ui.button("Select all").clicked() {
                    self.selected_lines = (0..self.lines.len()).collect();
                }
                let any_selected = !self.selected_lines.is_empty();
                ui.add_enabled_ui(any_selected, |ui| {
                    if ui.button("Deselect all").clicked() {
                        self.selected_lines.clear();
                    }
                });
                ui.label(format!("{} selected", self.selected_lines.len()));
                ui.separator();
                ui.add_enabled_ui(any_selected, |ui| {
                    ui.label("Set voice for selection:");
                    for singer in Singer::ALL {
                        if ui.button(singer.label()).clicked() {
                            let n = self.selected_lines.len();
                            for &i in &self.selected_lines {
                                if let Some(line) = self.lines.get_mut(i) {
                                    line.singer = singer;
                                    // Plainly setting a base voice also clears
                                    // any custom name, same as the per-line
                                    // dropdown - otherwise the selection would
                                    // end up showing a stale custom label that
                                    // no longer matches what was just set.
                                    line.custom_singer_name = None;
                                }
                            }
                            self.status =
                                format!("Set {n} selected line(s) to {}.", singer.label());
                        }
                    }
                    let known_custom = self.known_custom_singers();
                    if !known_custom.is_empty() {
                        ui.separator();
                        egui::ComboBox::from_id_source("bulk_custom_singer")
                            .width(100.0)
                            .selected_text("Custom…")
                            .show_ui(ui, |ui| {
                                for (name, base) in &known_custom {
                                    if ui.selectable_label(false, name).clicked() {
                                        let n = self.selected_lines.len();
                                        for &i in &self.selected_lines {
                                            if let Some(line) = self.lines.get_mut(i) {
                                                line.singer = *base;
                                                line.custom_singer_name = Some(name.clone());
                                            }
                                        }
                                        self.status =
                                            format!("Set {n} selected line(s) to {name}.");
                                    }
                                }
                            });
                    }
                });
            });
            ui.horizontal(|ui| {
                let any_selected = !self.selected_lines.is_empty();
                ui.add_enabled_ui(any_selected, |ui| {
                    ui.label("Shift selection:");
                    ui.add(
                        egui::DragValue::new(&mut self.shift_amount_secs)
                            .range(0.01..=30.0)
                            .speed(0.01)
                            .fixed_decimals(2)
                            .suffix(" s"),
                    );
                    let mut delta = None;
                    if ui
                        .button("◀ Earlier")
                        .on_hover_text(
                            "Move every selected line (start, end, and word timing) earlier \
                             by this amount. Handy for a whole song or section that was \
                             tapped consistently late.",
                        )
                        .clicked()
                    {
                        delta = Some(-self.shift_amount_secs);
                    }
                    if ui
                        .button("Later ▶")
                        .on_hover_text(
                            "Move every selected line (start, end, and word timing) later by \
                             this amount.",
                        )
                        .clicked()
                    {
                        delta = Some(self.shift_amount_secs);
                    }
                    if let Some(delta) = delta {
                        let indices: Vec<usize> = self.selected_lines.iter().copied().collect();
                        self.status = match lyrics::shift_lines(&mut self.lines, &indices, delta) {
                            Ok(n) => format!(
                                "Shifted {n} line(s) {} by {:.2}s.",
                                if delta < 0.0 { "earlier" } else { "later" },
                                delta.abs()
                            ),
                            Err(e) => e,
                        };
                    }
                });
                ui.separator();
                let all_timed = !self.selected_lines.is_empty()
                    && self.selected_lines.iter().all(|&i| {
                        self.lines.get(i).is_some_and(|l| l.start.is_some())
                    });
                ui.add_enabled_ui(all_timed, |ui| {
                    if ui
                        .button("📋 Copy timing to…")
                        .on_hover_text(
                            "Copy the selected (timed) lines' timing onto a repeat of them - a \
                             second chorus sung to the same rhythm. Every line and word lands \
                             at the same spacing; you just set where the repeat starts.",
                        )
                        .clicked()
                    {
                        self.open_copy_timing_dialog();
                    }
                });
            });
            ui.horizontal(|ui| {
                help_text(ui, self.prefs.show_tips, "Shift-click selects a range.", "Click rows' checkboxes to select/deselect them one at a time - \
                         Shift-click for a range.");
                ui.checkbox(&mut self.compact_timing_table, "Compact table")
                    .on_hover_text(
                        "Truncate long lyric lines in the table below (hover a truncated \
                         one to see the full text) instead of letting them widen the table.",
                    );
                ui.checkbox(&mut self.follow_timing_table, "Follow current line")
                    .on_hover_text(
                        "While the song plays, keep the line being timed scrolled into view \
                         in the table below. You can still scroll by hand - it only moves \
                         again when the next line comes up.",
                    );
            });

            // Which row (if any) the table should bring into view this
            // frame: the next line to tap while tapping along, otherwise
            // the line the fine-tune panel is following. Only while
            // playing, and only when that row changes - so clicking a row
            // while paused, or scrolling by hand, never gets yanked away.
            let follow_target = if self.next_untimed < self.lines.len() {
                Some(self.next_untimed)
            } else {
                self.word_tap_line
            };
            let is_playing = self.audio.as_ref().is_some_and(|a| a.is_playing());
            let scroll_to_row = if self.follow_timing_table
                && is_playing
                && follow_target != self.timing_table_scrolled_to
            {
                follow_target
            } else {
                None
            };
            let scroll_to_row = self.timing_table_scroll_request.take().or(scroll_to_row);
            self.timing_table_scrolled_to = follow_target;

            egui::ScrollArea::vertical()
                .id_source("timing_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Current auto-estimated end per line (used only as a
                    // greyed-out placeholder hint in the End field, never as
                    // its actual displayed/editable value - showing the
                    // live estimate there made an untapped end look like it
                    // had already been set).
                    let (timed_for_grid, indices_for_grid) = self.resolved_with_indices();
                    let mut estimated_end = vec![None; self.lines.len()];
                    // The actual gap before each line starts (previous
                    // timed line's sing_end -> this line's start), used
                    // only to warn when "Force" won't have much room to
                    // work with - `None` for the very first timed line
                    // (nothing sung before it to measure from; the
                    // title-card-to-first-line gap is a separate
                    // computation this table doesn't have handy, so it's
                    // simplest to just not warn there).
                    let mut gap_before_line = vec![None; self.lines.len()];
                    let mut prev_sing_end: Option<f64> = None;
                    for (t, &orig_idx) in timed_for_grid.iter().zip(indices_for_grid.iter()) {
                        estimated_end[orig_idx] = Some(t.sing_end);
                        if let Some(prev) = prev_sing_end {
                            gap_before_line[orig_idx] = Some(t.start - prev);
                        }
                        prev_sing_end = Some(t.sing_end);
                    }
                    let (backing_for_grid, backing_indices_for_grid) =
                        lyrics::resolve_backing_track_indexed(&self.lines, &self.timing_settings);
                    for (t, &orig_idx) in backing_for_grid.iter().zip(backing_indices_for_grid.iter())
                    {
                        estimated_end[orig_idx] = Some(t.sing_end);
                    }

                    let mut retap_idx: Option<usize> = None;
                    let mut words_idx: Option<usize> = None;
                    let mut backing_toggle_idx: Option<usize> = None;
                    let mut nudge: Option<(usize, f64)> = None;
                    let mut clear_idx: Option<usize> = None;
                    let mut commit_start: Option<(usize, String)> = None;
                    let mut commit_end: Option<(usize, String)> = None;
                    let mut selection_click: Option<(usize, bool)> = None;

                    // Both grids below use this same minimum row height,
                    // so the sticky checkbox column's rows can't drift out
                    // of alignment with the scrollable "rest" grid's rows -
                    // every widget in both (checkbox, text edit, combo
                    // box, button) already naturally sizes to this same
                    // theme constant, so this is just an explicit
                    // guarantee, not a guess.
                    let row_height = ui.spacing().interact_size.y;

                    // Which row's action buttons to show: the hovered row
                    // (from last frame's row positions), plus whichever rows
                    // are in play anyway - see `row_active` below.
                    let hovered_row = ui.ctx().pointer_hover_pos().and_then(|p| {
                        if !self.table_clip.contains(p) {
                            return None;
                        }
                        self.table_row_spans.iter().position(|span| span.contains(p.y))
                    });
                    self.table_clip = ui.clip_rect();
                    let mut row_spans: Vec<egui::Rangef> = Vec::with_capacity(self.lines.len());
                    let statuses = line_statuses(&self.lines);
                    let half_gap = 2.0;

                    ui.horizontal_top(|ui| {
                        // Sticky first column: just the row-selection
                        // checkboxes, in their own fixed-width Grid outside
                        // the horizontally-scrolling one below - so you can
                        // always tell which rows are selected (and click to
                        // change that) no matter how far right you've
                        // scrolled to reach the Singer/Countdown/action
                        // columns. Both this and the "rest" grid below sit
                        // inside one shared *vertical*-only ScrollArea, so
                        // their rows always scroll in lockstep - the only
                        // independent scrolling is the "rest" grid's own
                        // horizontal one.
                        egui::Grid::new("lines_grid_sticky")
                            .num_columns(1)
                            .striped(true)
                            .spacing([8.0, 4.0])
                            .min_row_height(row_height)
                            .show(ui, |ui| {
                                ui.label("");
                                ui.end_row();

                                #[allow(clippy::needless_range_loop)]
                                for i in 0..self.lines.len() {
                                    // See `apply_row_selection_click` for
                                    // the click/shift-click semantics.
                                    let mut checked = self.selected_lines.contains(&i);
                                    let mut checkbox_resp = None;
                                    let cell = ui.horizontal(|ui| {
                                        let status = statuses[i];
                                        ui.label(
                                            egui::RichText::new("●")
                                                .color(status.color(ui.visuals())),
                                        )
                                        .on_hover_text(status.describe());
                                        checkbox_resp = Some(ui.checkbox(&mut checked, ""));
                                    });
                                    let r = cell.response.rect;
                                    row_spans.push(egui::Rangef::new(
                                        r.top() - half_gap,
                                        r.bottom() + half_gap,
                                    ));
                                    let checkbox_resp = checkbox_resp.expect("drawn above");
                                    // Auto-follow scrolls via this sticky
                                    // column (outside the horizontal
                                    // ScrollArea) so it only ever moves
                                    // the table vertically.
                                    if scroll_to_row == Some(i) {
                                        checkbox_resp.scroll_to_me(Some(egui::Align::Center));
                                    }
                                    if checkbox_resp.clicked() {
                                        let shift = ui.input(|inp| inp.modifiers.shift);
                                        selection_click = Some((i, shift));
                                    }
                                    ui.end_row();
                                }
                            });

                        egui::ScrollArea::horizontal()
                            .id_source("timing_scroll_h")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                egui::Grid::new("lines_grid_rest")
                                    .num_columns(9)
                                    .striped(true)
                                    .spacing([8.0, 4.0])
                                    .min_row_height(row_height)
                                    .show(ui, |ui| {
                                        ui.label(egui::RichText::new("Start").small().weak());
                                        ui.label(egui::RichText::new("Lyric").small().weak());
                                        ui.label(egui::RichText::new("End").small().weak());
                                        ui.label(egui::RichText::new("Singer").small().weak());
                                        ui.label(egui::RichText::new("Countdown").small().weak());
                                        ui.label("");
                                        ui.label("");
                                        ui.label("");
                                        ui.label("");
                                        ui.end_row();

                                        // Not a `.zip()`/`.enumerate()`
                                        // candidate: each iteration needs
                                        // the plain index `i` itself (to
                                        // defer mutation - see the
                                        // `retap_idx`/etc. handling below,
                                        // which avoids double-borrowing
                                        // `self` while its own fields are
                                        // being edited inline above), not
                                        // just a borrowed element.
                                        #[allow(clippy::needless_range_loop)]
                                        for i in 0..self.lines.len() {
                                let is_next = i == self.next_untimed;

                                // Start field.
                                let editing_start =
                                    self.start_edit.as_ref().map(|(idx, _)| *idx) == Some(i);
                                let mut start_buf = if editing_start {
                                    self.start_edit.as_ref().unwrap().1.clone()
                                } else {
                                    match self.lines[i].start {
                                        Some(s) => lyrics::format_timecode(s),
                                        None => String::new(),
                                    }
                                };
                                let start_resp = ui.add(
                                    egui::TextEdit::singleline(&mut start_buf)
                                        .desired_width(95.0)
                                        .hint_text("00:00.00"),
                                );
                                if start_resp.has_focus() {
                                    self.start_edit = Some((i, start_buf));
                                } else if start_resp.lost_focus() {
                                    commit_start = Some((i, start_buf));
                                    self.start_edit = None;
                                }

                                ui.scope(|ui| {
                                    // Compact mode: truncate (character
                                    // count, not pixel width - simpler and
                                    // good enough for a hover-to-see-more
                                    // affordance) instead of letting a long
                                    // line widen the whole table - see the
                                    // "Compact table" checkbox above.
                                    const COMPACT_LYRIC_MAX_CHARS: usize = 18;
                                    let full_text = self.lines[i].text.as_str();
                                    let truncated = self.compact_timing_table
                                        && full_text.chars().count() > COMPACT_LYRIC_MAX_CHARS;
                                    let shown_text: std::borrow::Cow<str> = if truncated {
                                        let head: String =
                                            full_text.chars().take(COMPACT_LYRIC_MAX_CHARS).collect();
                                        std::borrow::Cow::Owned(format!("{head}…"))
                                    } else {
                                        std::borrow::Cow::Borrowed(full_text)
                                    };
                                    ui.set_max_width(if self.compact_timing_table {
                                        90.0
                                    } else {
                                        220.0
                                    });
                                    // Backing lines are indented and italic so
                                    // the second voice reads as tucked in under
                                    // the main lyrics around it.
                                    let shown_text: std::borrow::Cow<str> =
                                        if self.lines[i].is_backing {
                                            std::borrow::Cow::Owned(format!("↳ {shown_text}"))
                                        } else {
                                            shown_text
                                        };
                                    let text_label = if is_next {
                                        egui::RichText::new(shown_text.as_ref()).strong()
                                    } else {
                                        egui::RichText::new(shown_text.as_ref())
                                    };
                                    let text_label = if self.lines[i].is_backing {
                                        text_label.italics()
                                    } else {
                                        text_label
                                    };
                                    // Clicking the lyric text itself opens/switches the
                                    // "Fine-tune words" panel to this line - the same as
                                    // the "Words" button below, just a bigger, more obvious
                                    // target, and one that also shows which line is
                                    // currently selected (highlighted) at a glance.
                                    let selected = self.word_tap_line == Some(i);
                                    let hover_text = if truncated {
                                        format!(
                                            "{full_text}\n\nClick to select this line for \
                                             fine-tuning words."
                                        )
                                    } else {
                                        "Click to select this line for fine-tuning words."
                                            .to_string()
                                    };
                                    if ui
                                        .selectable_label(selected, text_label)
                                        .on_hover_text(hover_text)
                                        .clicked()
                                    {
                                        words_idx = Some(i);
                                    }
                                });

                                // End field - stays empty (just like Start
                                // does before its first tap) until this
                                // line actually has an explicit end, so
                                // tapping the start alone never makes it
                                // look like the end was set too.
                                let editing_end =
                                    self.end_edit.as_ref().map(|(idx, _)| *idx) == Some(i);
                                let mut end_buf = if editing_end {
                                    self.end_edit.as_ref().unwrap().1.clone()
                                } else {
                                    match self.lines[i].sing_end_override {
                                        Some(e) => lyrics::format_timecode(e),
                                        None => String::new(),
                                    }
                                };
                                let end_hint = match estimated_end[i] {
                                    Some(e) => format!("auto {}", lyrics::format_timecode(e)),
                                    None => "00:00.00".to_string(),
                                };
                                let end_resp = ui.add(
                                    egui::TextEdit::singleline(&mut end_buf)
                                        .desired_width(95.0)
                                        .hint_text(end_hint),
                                );
                                if end_resp.has_focus() {
                                    self.end_edit = Some((i, end_buf));
                                } else if end_resp.lost_focus() {
                                    commit_end = Some((i, end_buf));
                                    self.end_edit = None;
                                }

                                // Both widgets below share this one cell (like every
                                // other column in this grid, exactly one widget/group
                                // per cell) - the grid has a fixed `.num_columns(9)`
                                // matching the header row above, so adding the custom-
                                // name text field as a *second* top-level widget here
                                // (instead of nesting it in this same `horizontal`)
                                // would silently push every later cell one column out
                                // of alignment with its own header.
                                ui.horizontal(|ui| {
                                    let known_custom = self.known_custom_singers();
                                    egui::ComboBox::from_id_source(("singer", i))
                                        .width(82.0)
                                        .selected_text(lyrics::effective_singer_label(
                                            self.lines[i].singer,
                                            &self.lines[i].custom_singer_name,
                                        ))
                                        .show_ui(ui, |ui| {
                                            // The 5 base categories - picking one of these
                                            // plainly also clears any custom name, so the
                                            // dropdown's own selected-text stays consistent
                                            // with what was actually just picked (a manual
                                            // click check, not `selectable_value`, since a
                                            // single click here needs to set *two* fields).
                                            for (base, label) in [
                                                (Singer::Default, "Default"),
                                                (Singer::Male, "Male"),
                                                (Singer::Female, "Female"),
                                                (Singer::Duet, "Duet"),
                                                (Singer::Screaming, "Screaming"),
                                                (Singer::Backing, "Backing"),
                                            ] {
                                                let selected = self.lines[i].singer == base
                                                    && self.lines[i].custom_singer_name.is_none();
                                                if ui.selectable_label(selected, label).clicked() {
                                                    self.lines[i].singer = base;
                                                    self.lines[i].custom_singer_name = None;
                                                }
                                            }
                                            if !known_custom.is_empty() {
                                                ui.separator();
                                                for (name, base) in &known_custom {
                                                    let selected = self.lines[i].singer == *base
                                                        && self.lines[i].custom_singer_name.as_ref()
                                                            == Some(name);
                                                    if ui.selectable_label(selected, name).clicked()
                                                    {
                                                        self.lines[i].singer = *base;
                                                        self.lines[i].custom_singer_name =
                                                            Some(name.clone());
                                                    }
                                                }
                                            }
                                        });
                                    // Always-visible, separate from the dropdown above:
                                    // types a new custom name (or edits this line's
                                    // existing one) without touching which base color it
                                    // uses - see `LyricLine::custom_singer_name`'s own
                                    // docs. Typing a genuinely new name here is what makes
                                    // it show up as a reusable dropdown entry (via
                                    // `known_custom_singers`) on every other line too, not
                                    // just this one.
                                    let mut custom_buf = self.lines[i]
                                        .custom_singer_name
                                        .clone()
                                        .unwrap_or_default();
                                    if ui
                                        .add(
                                            egui::TextEdit::singleline(&mut custom_buf)
                                                .desired_width(100.0)
                                                .hint_text("Custom name"),
                                        )
                                        .changed()
                                    {
                                        self.lines[i].custom_singer_name =
                                            if custom_buf.trim().is_empty() {
                                                None
                                            } else {
                                                Some(custom_buf)
                                            };
                                    }
                                });

                                ui.horizontal(|ui| {
                                    egui::ComboBox::from_id_source(("countdown_mode", i))
                                        .width(78.0)
                                        .selected_text(self.lines[i].countdown_mode.label())
                                        .show_ui(ui, |ui| {
                                            for mode in lyrics::CountdownMode::ALL {
                                                ui.selectable_value(
                                                    &mut self.lines[i].countdown_mode,
                                                    mode,
                                                    mode.label(),
                                                );
                                            }
                                        });
                                    // A short/nonexistent gap before this
                                    // line means a forced countdown has to
                                    // compress to fit (or, for a zero/
                                    // negative gap, won't show at all - see
                                    // lyrics.rs's countdown_window_between)
                                    // - flagged here rather than silently
                                    // letting the user wonder why "Force"
                                    // didn't visibly change anything.
                                    let short_gap = self.lines[i].countdown_mode
                                        == lyrics::CountdownMode::Force
                                        && gap_before_line[i]
                                            .is_some_and(|g| g < lyrics::COUNTDOWN_LEAD_SECS);
                                    if short_gap {
                                        ui.label(
                                            egui::RichText::new("⚠")
                                                .color(egui::Color32::from_rgb(230, 140, 40)),
                                        )
                                        .on_hover_text(
                                            "This line's gap is too short for the full \
                                             countdown - it'll compress to fit (or not show \
                                             at all, if there's no gap left).",
                                        );
                                    }
                                });

                                // Action buttons only on the active row - hidden
                                // (space kept, so nothing shifts) elsewhere.
                                let row_active = hovered_row == Some(i)
                                    || self.word_tap_line == Some(i)
                                    || self.next_untimed == i
                                    || self.selected_lines.contains(&i);
                                ui.add_visible_ui(row_active, |ui| {
                                if self.lines[i].is_backing {
                                    // Backing lines aren't part of tap-along,
                                    // so their own Tap button does both ends:
                                    // see `tap_backing_line`.
                                    let wants_end = self.lines[i].start.is_some()
                                        && self.lines[i].sing_end_override.is_none();
                                    let label = if wants_end { "Tap end" } else { "Tap start" };
                                    if ui
                                        .small_button(label)
                                        .on_hover_text(
                                            "While the song plays: tap when the backing vocal \
                                             starts, then again when it ends. Backing lines are \
                                             skipped by tap-along, so time them here (a second \
                                             play-through works well).",
                                        )
                                        .clicked()
                                    {
                                        retap_idx = Some(i);
                                    }
                                } else if ui.small_button("Tap").clicked() {
                                    retap_idx = Some(i);
                                }
                                });
                                ui.add_visible_ui(row_active, |ui| {
                                if ui.small_button("Words").clicked() {
                                    words_idx = Some(i);
                                }
                                });
                                ui.add_visible_ui(row_active, |ui| {
                                if ui
                                    .selectable_label(self.lines[i].is_backing, "Backing")
                                    .on_hover_text(
                                        "Make this a backing vocal: a second voice singing \
                                         different lyrics over the main line(s). It's timed \
                                         independently and can overlap any main line, and \
                                         shows in its own lane below the main lyrics. Click \
                                         again to put it back on the main track.",
                                    )
                                    .clicked()
                                {
                                    backing_toggle_idx = Some(i);
                                }
                                });
                                ui.add_visible_ui(row_active, |ui| {
                                ui.horizontal(|ui| {
                                    if self.lines[i].start.is_some() {
                                        if ui.small_button("-0.1s").clicked() {
                                            nudge = Some((i, -0.1));
                                        }
                                        if ui.small_button("+0.1s").clicked() {
                                            nudge = Some((i, 0.1));
                                        }
                                        if ui.small_button("✕").clicked() {
                                            clear_idx = Some(i);
                                        }
                                    }
                                });
                                });
                                ui.end_row();
                                        }
                                    });
                            });
                    });

                    self.table_row_spans = row_spans;

                    if let Some((i, shift)) = selection_click {
                                apply_row_selection_click(
                                    &mut self.selected_lines,
                                    &mut self.selection_anchor,
                                    i,
                                    shift,
                                );
                            }
                            if let Some(i) = retap_idx {
                                if self.lines[i].is_backing {
                                    self.tap_backing_line(i);
                                } else {
                                    self.retap_line(i);
                                }
                            }
                            if let Some(i) = words_idx {
                                self.word_tap_line = Some(i);
                            }
                            if let Some(i) = backing_toggle_idx {
                                self.toggle_backing_line(i);
                            }
                            if let Some((i, delta)) = nudge {
                                self.nudge_line_start(i, delta);
                            }
                            if let Some(i) = clear_idx {
                                self.lines[i].start = None;
                                self.lines[i].sing_end_override = None;
                                self.recompute_next_untimed();
                            }
                            if let Some((i, text)) = commit_start {
                                match lyrics::parse_timecode(&text) {
                                    Some(v) => {
                                        match lyrics::check_start_change(&self.lines, i, v) {
                                            Ok(()) => {
                                                self.lines[i].start = Some(v);
                                                self.recompute_next_untimed();
                                                self.audition_seek(v);
                                            }
                                            Err(e) => self.status = e,
                                        }
                                    }
                                    None if text.trim().is_empty() => {
                                        self.lines[i].start = None;
                                        self.recompute_next_untimed();
                                    }
                                    None => {
                                        self.status = format!(
                                            "Couldn't read \"{text}\" as a time (try MM:SS.SS)."
                                        );
                                    }
                                }
                            }
                            if let Some((i, text)) = commit_end {
                                match lyrics::parse_timecode(&text) {
                                    Some(v) => match lyrics::check_end_change(&self.lines, i, v) {
                                        Ok(()) => {
                                            self.lines[i].sing_end_override = Some(v);
                                            self.recompute_next_untimed();
                                            self.audition_seek(v);
                                        }
                                        Err(e) => self.status = e,
                                    },
                                    None if text.trim().is_empty() => {
                                        self.lines[i].sing_end_override = None;
                                        self.recompute_next_untimed();
                                    }
                                    None => {
                                        self.status = format!(
                                            "Couldn't read \"{text}\" as a time (try MM:SS.SS)."
                                        );
                                    }
                                }
                            }
                });
        });

        if self.preview_popped_out {
            self.show_preview_window(ctx);
        }

        // Record undo history for whatever changed this frame - see
        // `track_undo_history` for why a timeline drag or a focused text
        // field counts as one gesture rather than a step per frame.
        let gesture_active = self.timeline_drag.is_some() || ctx.memory(|m| m.focused().is_some());
        self.track_undo_history(gesture_active);
    }

    /// Called once on a clean shutdown (window closed, app quit normally -
    /// including an *accidental* close, which looks identical to a
    /// deliberate one from here, and a deliberate "Quit Without Saving"
    /// from the confirmation prompt). Only clears the crash-recovery
    /// autosave if there's nothing it would be protecting - see
    /// `has_unsaved_changes`. Otherwise the autosave is left in place, the
    /// same as if this had never run at all (a crash, a force-quit, a
    /// system shutdown) - so `KaraokeApp::new` finds it and offers to
    /// recover next launch regardless of *why* the work never got saved.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.pending_recovery.is_some() {
            // The recovery prompt itself was never answered (the window
            // was closed while it was still showing) - leave the autosave
            // exactly as found rather than evaluating it against the
            // still-empty live state, which would otherwise look "clean"
            // and wrongly delete the very data being offered for recovery.
            return;
        }
        if !self.has_unsaved_changes() {
            project::clear_autosave(APP_ID);
        }
    }
}

/// Passed to `eframe::run_native` as both the window title and (absent an
/// explicit `ViewportBuilder::app_id`) the id `eframe::storage_dir` uses to
/// find this app's per-user data directory - kept as one constant so the
/// autosave path and the window title can never drift apart.
const APP_ID: &str = "Abyssal CDG Creator";

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1350.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        APP_ID,
        options,
        Box::new(|_cc| Ok(Box::new(KaraokeApp::new()))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors_from(p: Palette) -> ExportColors {
        let c = color32_from_cdg;
        ExportColors {
            bg: c(p.background),
            default_unsung: c(p.default_unsung),
            default_highlight: c(p.default_highlight),
            male_unsung: c(p.male_unsung),
            male_highlight: c(p.male_highlight),
            female_unsung: c(p.female_unsung),
            female_highlight: c(p.female_highlight),
            duet_unsung: c(p.duet_unsung),
            duet_highlight: c(p.duet_highlight),
            preview: c(p.preview),
            title: c(p.title),
            artist: c(p.artist),
            screaming_unsung: c(p.screaming_unsung),
            screaming_highlight: c(p.screaming_highlight),
            backing_unsung: c(p.backing_unsung),
            backing_highlight: c(p.backing_highlight),
        }
    }

    #[test]
    fn built_in_presets_pass_every_readability_check() {
        for preset in ALL_COLOR_PRESETS {
            let warnings = color_warnings(&colors_from(preset.palette()), &Singer::ALL);
            assert!(warnings.is_empty(), "{}: {warnings:#?}", preset.label());
        }
    }

    #[test]
    fn color_checks_catch_collapsed_faint_and_clashing_colors() {
        let mut c = colors_from(Palette::default());
        // Two 8-bit colors that round to the same CDG shade.
        // 240 and 250 both round to shade 14 of 15; 10 rounds to 0.
        c.male_unsung = egui::Color32::from_rgb(240, 240, 0);
        c.male_highlight = egui::Color32::from_rgb(250, 250, 10);
        // Dark text on the dark background.
        c.female_unsung = egui::Color32::from_rgb(40, 0, 50);
        let w = color_warnings(&c, &[Singer::Male, Singer::Female]);
        assert!(
            w.iter()
                .any(|m| m.starts_with("Male: the upcoming and sung colors come out identical")),
            "{w:#?}"
        );
        assert!(
            w.iter()
                .any(|m| m.starts_with("Female upcoming text is hard to read")),
            "{w:#?}"
        );
        // Unused voices aren't checked.
        assert!(!w.iter().any(|m| m.contains("Screaming")));
        // Two voices singing in the same color.
        let mut c = colors_from(Palette::default());
        c.duet_highlight = c.male_highlight;
        let w = color_warnings(&c, &[Singer::Male, Singer::Duet]);
        assert!(
            w.iter()
                .any(|m| m.starts_with("Male and Duet sing in nearly the same color")),
            "{w:#?}"
        );
    }

    #[test]
    fn contrast_ratio_matches_wcag_reference_values() {
        let black = [0.0, 0.0, 0.0];
        let white = [255.0, 255.0, 255.0];
        assert!((contrast_ratio(black, white) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(white, white) - 1.0).abs() < 1e-6);
        // #777777 on white is ~4.48:1.
        assert!((contrast_ratio([119.0, 119.0, 119.0], white) - 4.48).abs() < 0.02);
    }

    #[test]
    fn line_statuses_mark_untimed_estimated_and_fine_tuned() {
        let mut untimed = LyricLine::new("not yet");
        untimed.start = None;
        let mut estimated = LyricLine::new("two words");
        estimated.start = Some(1.0);
        let mut tuned = LyricLine::new("two words");
        tuned.start = Some(3.0);
        tuned.word_overrides = vec![Some(3.0), Some(3.6)];
        let mut single = LyricLine::new("Yeah");
        single.start = Some(5.0);
        assert_eq!(
            line_statuses(&[untimed, estimated, tuned, single]),
            vec![
                LineStatus::Untimed,
                LineStatus::Estimated,
                LineStatus::FineTuned,
                // One word: nothing left to fine-tune.
                LineStatus::FineTuned,
            ]
        );
    }

    #[test]
    fn shortcut_cheat_sheet_has_no_duplicate_keys() {
        let mut seen = std::collections::HashSet::new();
        for (_, keys) in SHORTCUTS {
            for (key, what) in *keys {
                assert!(!what.is_empty());
                assert!(seen.insert(*key), "{key} listed twice");
            }
        }
        // The new shortcuts are documented.
        for key in [", / .", "Enter", "Delete", "Ctrl+E", "Home", "F1 or ?"] {
            assert!(seen.contains(key), "{key} missing from the cheat sheet");
        }
    }

    #[test]
    fn jump_times_accept_the_common_forms() {
        assert_eq!(parse_jump_time("83"), Some(83.0));
        assert_eq!(parse_jump_time(" 1:23.5 "), Some(83.5));
        assert_eq!(parse_jump_time("01:02:03"), Some(3723.0));
        assert_eq!(parse_jump_time("-5"), None);
        assert_eq!(parse_jump_time("abc"), None);
        assert_eq!(parse_jump_time("1:2:3:4"), None);
        assert_eq!(parse_jump_time("nan"), None);
    }

    #[test]
    fn status_messages_are_classified_by_wording() {
        use StatusKind::*;
        let cases = [
            ("Export complete!\nCDG file: saved to /x.cdg", Success),
            (
                "Export finished with some errors:\nVideo: failed - boom",
                Warning,
            ),
            ("Export failed:\nCDG file: failed - disk full", Error),
            ("Couldn't load audio: no such file", Error),
            (
                "Start (00:01.00) can't be before the previous line starts (00:02.00).",
                Error,
            ),
            ("Press Play first, then tap along with the song.", Warning),
            ("Choose an output folder first.", Warning),
            (
                "Loaded project, but its audio file wasn't found at /a.mp3 - load it manually.",
                Warning,
            ),
            ("Saved project to /tmp/song.abyzl", Success),
            ("Auto-aligned 40 word(s) across 9 line(s).", Success),
            ("Line 3 ended at 00:12.40.", Info),
            ("Looping line 4 - press L or Loop line again to stop.", Info),
            ("Loop end (B) must be after the start (A).", Error),
            ("", Info),
        ];
        for (msg, want) in cases {
            assert_eq!(classify_status(msg), want, "{msg}");
        }
    }

    #[test]
    fn only_non_routine_statuses_get_a_toast() {
        assert_eq!(StatusKind::Info.toast_secs(), None);
        assert!(StatusKind::Error.toast_secs() > StatusKind::Success.toast_secs());
    }

    #[test]
    fn step_bar_walks_the_required_steps_in_order() {
        use StepStatus::*;
        let mut snap = WorkflowSnapshot::default();
        let (st, next) = workflow_status(&snap);
        assert_eq!(next, WorkflowStep::Audio);
        assert_eq!(st, [Current, Todo, Todo, Optional, Todo]);

        snap.has_audio = true;
        assert_eq!(workflow_status(&snap).1, WorkflowStep::Lyrics);

        snap.lines = 4;
        snap.timed = 2;
        let (st, next) = workflow_status(&snap);
        assert_eq!(next, WorkflowStep::Tap);
        assert_eq!(st[..3], [Done, Done, Current]);

        // All timed, words still estimated: Export is next, Fine-tune is
        // optional and doesn't block it.
        snap.timed = 4;
        snap.estimated = 3;
        let (st, next) = workflow_status(&snap);
        assert_eq!(next, WorkflowStep::Export);
        assert_eq!(st[3], Optional);
        assert_eq!(st[4], Current);

        snap.estimated = 0;
        snap.exported = true;
        let (st, _) = workflow_status(&snap);
        assert_eq!(st, [Done; 5]);
    }

    #[test]
    fn step_bar_never_skips_audio_even_with_timed_lyrics() {
        // e.g. a project whose audio file went missing.
        let snap = WorkflowSnapshot {
            lines: 3,
            timed: 3,
            ..Default::default()
        };
        let (st, next) = workflow_status(&snap);
        assert_eq!(next, WorkflowStep::Audio);
        assert_eq!(st[0], StepStatus::Current);
        assert_eq!(st[2], StepStatus::Done);
    }

    /// A minimal saved project (old-style file, defaults for everything
    /// newer) with the given title, audio path and lines.
    fn write_test_project(
        dir: &Path,
        name: &str,
        title: &str,
        audio: Option<&Path>,
        lines: &[LyricLine],
    ) -> PathBuf {
        let c = serde_json::json!({ "r": 200, "g": 200, "b": 200 });
        let json = serde_json::json!({
            "version": 1,
            "audio_path": audio,
            "lyrics_raw": lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>().join("\n"),
            "lines": lines,
            "title": title,
            "artist": "Band",
            "colors": {
                "background": c, "male_unsung": c, "male_highlight": c,
                "female_unsung": c, "female_highlight": c, "duet_unsung": c,
                "duet_highlight": c, "preview": c, "title": c, "artist": c,
                "screaming_unsung": c, "screaming_highlight": c
            },
            "video_resolution": "Hd1080",
        });
        let path = dir.join(format!("{name}.{}", project::FILE_EXTENSION));
        std::fs::write(&path, serde_json::to_string(&json).unwrap()).unwrap();
        path
    }

    fn timed_line(text: &str, start: f64, end: f64) -> LyricLine {
        let mut l = LyricLine::new(text);
        l.start = Some(start);
        l.sing_end_override = Some(end);
        l
    }

    fn text_only_options() -> ExportOptions {
        ExportOptions {
            cdg: true,
            lrc: true,
            ultrastar: false,
            ass: false,
            srt: true,
            video: false,
            instrumental: false,
            vocals: false,
            zip: false,
            zip_instrumental: false,
            lrc_enhanced: true,
            remove_vocals_for_video: false,
            instrumental_ext: "mp3",
        }
    }

    #[test]
    fn batch_export_writes_each_song_and_skips_ones_that_cannot_export() {
        let dir = std::env::temp_dir().join(format!("abyssal-batch-test-{}", std::process::id()));
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let a = write_test_project(
            &dir,
            "a",
            "First Song",
            None,
            &[timed_line("hello there", 1.0, 3.0)],
        );
        // Same title -> same default file name: must not overwrite "a".
        let b = write_test_project(
            &dir,
            "b",
            "First Song",
            None,
            &[timed_line("another one", 2.0, 4.0)],
        );
        let untimed =
            write_test_project(&dir, "c", "Untimed", None, &[LyricLine::new("not tapped")]);
        let missing = dir.join("nope.abyzl");

        let progress = std::cell::RefCell::new(Vec::new());
        let outcomes = run_batch_export(
            &[a, b, untimed, missing],
            &text_only_options(),
            &out,
            &|p| progress.borrow_mut().push(p),
        );

        for f in [
            "First Song.cdg",
            "First Song.lrc",
            "First Song.srt",
            "First Song (2).cdg",
            "First Song (2).lrc",
        ] {
            assert!(
                out.join(f).is_file(),
                "{f} missing; outcomes: {:?}",
                outcomes
                    .iter()
                    .map(|o| (&o.label, &o.result))
                    .collect::<Vec<_>>()
            );
        }
        let lrc = std::fs::read_to_string(out.join("First Song (2).lrc")).unwrap();
        assert!(lrc.contains("another"), "{lrc}");
        // Every output of the two good songs succeeded, labeled per song.
        let ok: Vec<_> = outcomes.iter().filter(|o| o.result.is_ok()).collect();
        assert_eq!(ok.len(), 6);
        assert!(ok
            .iter()
            .all(|o| o.label.starts_with("a - ") || o.label.starts_with("b - ")));
        // The untimed and unreadable projects were skipped with a reason.
        let skipped: Vec<_> = outcomes.iter().filter(|o| o.result.is_err()).collect();
        assert_eq!(skipped.len(), 2);
        assert!(skipped.iter().any(|o| o.label == "c"
            && o.result
                .as_ref()
                .unwrap_err()
                .contains("No lines are timed")));
        assert!(
            skipped
                .iter()
                .any(|o| o.label == "nope"
                    && o.result.as_ref().unwrap_err().contains("couldn't open"))
        );
        // Progress only moves forward and finishes at 100%.
        let p = progress.borrow();
        assert!(p.windows(2).all(|w| w[1] >= w[0]), "{p:?}");
        assert_eq!(p.last(), Some(&1.0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_skips_a_song_whose_audio_is_needed_but_missing() {
        let dir = std::env::temp_dir().join(format!("abyssal-batch-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = write_test_project(
            &dir,
            "song",
            "Song",
            Some(Path::new("/definitely/not/here.mp3")),
            &[timed_line("hi there", 1.0, 2.0)],
        );
        let opts = ExportOptions {
            video: true,
            ..text_only_options()
        };
        let outcomes = run_batch_export(&[p], &opts, &dir, &|_| {});
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0]
            .result
            .as_ref()
            .unwrap_err()
            .contains("audio file wasn't found"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_names_default_and_stay_unique() {
        assert_eq!(
            default_export_base_name(Some(Path::new("/m/Artist - Song.flac")), "T"),
            "Artist - Song"
        );
        assert_eq!(default_export_base_name(None, "  My Title "), "My Title");
        assert_eq!(default_export_base_name(None, "   "), "karaoke");
        let mut used = std::collections::HashSet::new();
        assert_eq!(unique_base_name("Song", &mut used), "Song");
        assert_eq!(unique_base_name("song", &mut used), "song (2)");
        assert_eq!(unique_base_name("Song", &mut used), "Song (3)");
    }

    #[test]
    fn project_colors_convert_the_same_way_loading_a_project_does() {
        // Old file without Default/Backing colors: Default falls back to
        // Male's, Backing to the default palette's.
        let c = project::RgbColor {
            r: 10,
            g: 20,
            b: 30,
        };
        let colors = project::ProjectColors {
            background: c,
            default_unsung: None,
            default_highlight: None,
            male_unsung: project::RgbColor { r: 1, g: 2, b: 3 },
            male_highlight: c,
            female_unsung: c,
            female_highlight: c,
            duet_unsung: c,
            duet_highlight: c,
            preview: c,
            title: c,
            artist: c,
            screaming_unsung: c,
            screaming_highlight: c,
            backing_unsung: None,
            backing_highlight: None,
        };
        let e = ExportColors::from_project(&colors);
        assert_eq!(e.default_unsung, e.male_unsung);
        assert_eq!(
            e.backing_unsung,
            color32_from_cdg(Palette::default().backing_unsung)
        );
    }

    #[test]
    fn loop_wraps_only_on_a_forward_crossing_of_its_end() {
        assert!(loop_should_wrap(Some(9.98), 10.01, 10.0));
        assert!(!loop_should_wrap(Some(9.0), 9.5, 10.0));
        // Seeking past the end by hand: already beyond it last frame.
        assert!(!loop_should_wrap(Some(30.0), 30.02, 10.0));
        // First frame after starting/seeking never wraps.
        assert!(!loop_should_wrap(None, 12.0, 10.0));
    }

    #[test]
    fn lyrics_edit_tracking_finds_a_pasted_block() {
        // "w\nx\n" then "y\nz\n" pasted at offset 4 (the start of line 2).
        let before = "w\nx\ny\nz";
        let after = "w\nx\ny\nz\ny\nz";
        let span = track_lyrics_edit(None, 8, 4);
        assert_eq!(span, 4..8);
        assert_eq!(char_span_to_line_range(after, span), 2..4);
        // Typing a further character earlier on shifts the span along.
        let span = track_lyrics_edit(Some(4..8), 1, 1);
        assert_eq!(span, 0..9);
        // A pure deletion still marks the line it happened on.
        assert_eq!(char_span_to_line_range(before, 2..2), 1..2);
    }

    #[test]
    fn sanitize_export_base_name_strips_directory_components_and_traversal() {
        // The actual attack this defends against: a project file's `title`
        // (which export_base_name can be auto-filled from) pointing
        // straight at an absolute path or a traversal sequence, which -
        // unsanitized - would let a shared .abyzl project file redirect
        // every export output away from the folder actually picked.
        assert_eq!(
            sanitize_export_base_name("/etc/cron.d/evil"),
            "evil",
            "an absolute path must be reduced to just its final component"
        );
        assert_eq!(
            sanitize_export_base_name("../../../elsewhere"),
            "elsewhere",
            "a relative traversal sequence must be reduced to just its final component"
        );
        assert_eq!(
            sanitize_export_base_name("foo/../bar"),
            "bar",
            "traversal in the middle of a path must still only keep the final component"
        );
    }

    #[test]
    fn sanitize_export_base_name_falls_back_to_karaoke_when_nothing_usable_remains() {
        assert_eq!(sanitize_export_base_name(""), "karaoke");
        assert_eq!(sanitize_export_base_name("   "), "karaoke");
        assert_eq!(sanitize_export_base_name("/"), "karaoke");
        assert_eq!(sanitize_export_base_name(".."), "karaoke");
    }

    #[test]
    fn sanitize_export_base_name_leaves_an_ordinary_name_untouched() {
        assert_eq!(sanitize_export_base_name("My Song Title"), "My Song Title");
        assert_eq!(sanitize_export_base_name("  spaced out  "), "spaced out");
    }

    #[test]
    fn parses_artist_and_title_from_the_expected_naming_scheme() {
        let (artist, title) =
            parse_artist_title_from_filename(Path::new("Imagine Dragons - Believer.mp3")).unwrap();
        assert_eq!(artist, "Imagine Dragons");
        assert_eq!(title, "Believer");
    }

    #[test]
    fn trims_extra_whitespace_around_the_separator() {
        let (artist, title) =
            parse_artist_title_from_filename(Path::new("  Queen  -  Bohemian Rhapsody.flac"))
                .unwrap();
        assert_eq!(artist, "Queen");
        assert_eq!(title, "Bohemian Rhapsody");
    }

    #[test]
    fn returns_none_without_a_dash_separator() {
        assert!(parse_artist_title_from_filename(Path::new("Believer.mp3")).is_none());
    }

    #[test]
    fn returns_none_when_one_side_is_empty() {
        assert!(parse_artist_title_from_filename(Path::new(" - Believer.mp3")).is_none());
        assert!(parse_artist_title_from_filename(Path::new("Imagine Dragons - .mp3")).is_none());
    }

    #[test]
    fn only_splits_on_the_first_dash() {
        // A song whose real title contains " - " should still split into
        // (artist, everything else) rather than truncating at the last one.
        let (artist, title) =
            parse_artist_title_from_filename(Path::new("Artist - Part One - Part Two.mp3"))
                .unwrap();
        assert_eq!(artist, "Artist");
        assert_eq!(title, "Part One - Part Two");
    }

    // --- apply_row_selection_click ---------------------------------------

    #[test]
    fn plain_clicks_accumulate_a_multi_line_selection() {
        let mut selected = std::collections::BTreeSet::new();
        let mut anchor = None;
        apply_row_selection_click(&mut selected, &mut anchor, 1, false);
        apply_row_selection_click(&mut selected, &mut anchor, 4, false);
        apply_row_selection_click(&mut selected, &mut anchor, 9, false);
        assert_eq!(selected, std::collections::BTreeSet::from([1, 4, 9]));
        assert_eq!(anchor, Some(9));
    }

    #[test]
    fn shift_click_selects_the_inclusive_range_from_the_anchor() {
        let mut selected = std::collections::BTreeSet::new();
        let mut anchor = Some(2);
        apply_row_selection_click(&mut selected, &mut anchor, 5, true);
        assert_eq!(selected, std::collections::BTreeSet::from([2, 3, 4, 5]));

        // Works in either direction (clicking above the anchor, not just below).
        let mut anchor2 = Some(5);
        let mut selected2 = std::collections::BTreeSet::new();
        apply_row_selection_click(&mut selected2, &mut anchor2, 2, true);
        assert_eq!(selected2, std::collections::BTreeSet::from([2, 3, 4, 5]));
    }

    #[test]
    fn shift_click_replaces_rather_than_extends_a_prior_selection() {
        let mut selected = std::collections::BTreeSet::from([0, 10, 20]);
        let mut anchor = Some(0);
        apply_row_selection_click(&mut selected, &mut anchor, 2, true);
        assert_eq!(selected, std::collections::BTreeSet::from([0, 1, 2]));
    }

    #[test]
    fn shift_click_with_no_prior_anchor_just_selects_the_clicked_line() {
        let mut selected = std::collections::BTreeSet::new();
        let mut anchor = None;
        apply_row_selection_click(&mut selected, &mut anchor, 3, true);
        assert_eq!(selected, std::collections::BTreeSet::from([3]));
        assert_eq!(anchor, Some(3));
    }

    #[test]
    fn plain_click_toggles_one_line_without_touching_the_rest() {
        let mut selected = std::collections::BTreeSet::from([1, 2]);
        let mut anchor = Some(2);

        // Not selected yet -> adds it.
        apply_row_selection_click(&mut selected, &mut anchor, 5, false);
        assert_eq!(selected, std::collections::BTreeSet::from([1, 2, 5]));

        // Already selected -> removes just that one.
        apply_row_selection_click(&mut selected, &mut anchor, 2, false);
        assert_eq!(selected, std::collections::BTreeSet::from([1, 5]));
    }

    #[test]
    fn any_click_moves_the_anchor_to_the_clicked_line() {
        let mut selected = std::collections::BTreeSet::new();
        let mut anchor = Some(0);
        apply_row_selection_click(&mut selected, &mut anchor, 7, false);
        assert_eq!(anchor, Some(7));
    }
}
