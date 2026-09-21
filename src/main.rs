// Windows GUI apps default to the "console" subsystem, which pops up (and
// keeps open) a terminal window behind the app for the lifetime of the
// process. Switching to the "windows" subsystem in release builds hides
// that window - debug builds keep the console so `println!`/panic output
// is still visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod align;
mod audio;
mod cdg;
mod export;
mod font;
mod formats;
mod lyrics;
mod project;
mod recent;
mod timeline;
mod video;
mod vocals;
mod waveform;

use audio::AudioPlayer;
use eframe::egui;
use export::Palette;
use lyrics::{
    blank_sung_lines, countdown_window, group_into_blocks, hide_upcoming_lines,
    parse_pasted_lyrics, resolve_timing, singer_legend, LyricLine, Singer, TimedLine,
};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use video::{Resolution, VideoPalette};

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

struct KaraokeApp {
    audio: Option<AudioPlayer>,
    audio_error: Option<String>,

    lyrics_raw: String,
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
    /// Line index + in-progress text while a start-time field in the
    /// timing table is focused - `None` the rest of the time, so the
    /// displayed text otherwise always mirrors the line's live value.
    start_edit: Option<(usize, String)>,
    /// Same as `start_edit`, for the end-time field.
    end_edit: Option<(usize, String)>,

    title: String,
    artist: String,

    color_bg: egui::Color32,
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
    /// Cached preview texture for `background`/`background_fit`, alongside
    /// the values it was built from so the (fairly expensive - image
    /// decode, or an `ffmpeg` round trip for a video's poster frame)
    /// regeneration only happens when either actually changes, not every
    /// frame. The inner `None` means "already tried and failed to load
    /// `background`" (e.g. a moved/deleted file) - remembered so a broken
    /// background doesn't retry that expensive load every single frame.
    background_preview: Option<(
        video::Background,
        video::BackgroundFit,
        Option<egui::TextureHandle>,
    )>,
    /// Whether a video export should mux in a vocals-reduced copy of the
    /// audio (via [`vocals::remove_vocals_to_file`]) instead of the
    /// original.
    remove_vocals_for_video: bool,
    /// Whether the "Export…" options window is open.
    show_export_dialog: bool,
    export_cdg: bool,
    export_lrc: bool,
    export_ultrastar: bool,
    export_video: bool,
    export_instrumental: bool,
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

    /// Language code passed to aeneas for auto-align (see `align.rs`) -
    /// defaults to English since the app has no other language-awareness.
    align_language: String,
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
    HighContrast,
    Sunset,
    Ocean,
}

impl ColorPreset {
    fn label(self) -> &'static str {
        match self {
            Self::Classic => "Classic",
            Self::HighContrast => "High Contrast",
            Self::Sunset => "Sunset",
            Self::Ocean => "Ocean",
        }
    }

    fn palette(self) -> Palette {
        match self {
            Self::Classic => Palette::default(),
            Self::HighContrast => Palette::high_contrast(),
            Self::Sunset => Palette::sunset(),
            Self::Ocean => Palette::ocean(),
        }
    }
}
const ALL_COLOR_PRESETS: [ColorPreset; 4] = [
    ColorPreset::Classic,
    ColorPreset::HighContrast,
    ColorPreset::Sunset,
    ColorPreset::Ocean,
];

/// One output's result from a combined export - a combined export can
/// partially succeed (e.g. ffmpeg missing but audio-separator present), so
/// each requested output is tracked and reported independently.
struct ExportOutcome {
    label: &'static str,
    result: Result<PathBuf, String>,
}

/// Shared state for a combined export (any mix of .cdg/.mp4/instrumental
/// audio) running on a background thread, so the GUI stays responsive and
/// can show one progress bar spanning every selected output.
struct CombinedExportHandle {
    progress: Arc<AtomicU32>, // 0..=1000 (tenths of a percent), across all selected outputs
    result: Arc<Mutex<Option<Vec<ExportOutcome>>>>,
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

/// Shared state for a background `waveform::build_waveform` call, kicked
/// off whenever a new audio file is loaded - decoding a whole song can take
/// a noticeable fraction of a second, so it happens off the UI thread the
/// same way exports do.
struct WaveformJob {
    result: Arc<Mutex<Option<Result<waveform::Waveform, String>>>>,
}

/// Common language presets for the auto-align language picker - aeneas
/// (via eSpeak/eSpeak NG) supports many more than this by code alone; this
/// is just a convenient shortlist, not an exhaustive/validated list. The
/// text field next to it accepts any code directly.
const ALIGN_LANGUAGES: [(&str, &str); 9] = [
    ("eng", "English"),
    ("spa", "Spanish"),
    ("fra", "French"),
    ("deu", "German"),
    ("ita", "Italian"),
    ("por", "Portuguese"),
    ("jpn", "Japanese"),
    ("kor", "Korean"),
    ("cmn", "Mandarin"),
];

/// One already-timed line's forced-alignment result, as reported back from
/// the background thread [`KaraokeApp::start_word_alignment`] spawns - see
/// [`AlignJob`].
struct AlignOutcome {
    line_idx: usize,
    result: Result<Vec<align::WordAlignment>, String>,
}

/// A whole-job-level `Err` for something that stopped an auto-align run
/// before it could even attempt any line (aeneas not installed, no audio
/// loaded), or `Ok` with one [`AlignOutcome`] per line it tried (which can
/// still individually fail without taking down the rest of the run).
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
            current_project_path: None,
            pending_recovery: project::read_autosave(APP_ID),
            last_autosave: None,
            last_saved_snapshot: None,
            show_quit_confirm: false,
            quit_confirmed: false,
            lines: Vec::new(),
            next_untimed: 0,
            tap_phase: TapPhase::default(),
            word_tap_line: None,
            word_tap_mode: WordTapMode::default(),
            auto_follow_words: true,
            start_edit: None,
            end_edit: None,
            title: String::new(),
            artist: String::new(),
            color_bg: color32_from_cdg(p.background),
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
            color_preset: ColorPreset::default(),
            recent_files: recent::load(APP_ID),
            status: String::new(),
            seek_drag_value: None,
            video_resolution: Resolution::Hd1080,
            background: None,
            background_fit: video::BackgroundFit::default(),
            background_dim: 0.4,
            background_preview: None,
            remove_vocals_for_video: false,
            show_export_dialog: false,
            export_cdg: true,
            export_lrc: false,
            export_ultrastar: false,
            export_video: false,
            export_instrumental: false,
            lrc_enhanced_words: true,
            export_folder: None,
            export_base_name: String::new(),
            instrumental_format: InstrumentalFormat::default(),
            combined_export: None,
            timeline_view: timeline::View::default(),
            timeline_drag: None,
            waveform: None,
            waveform_job: None,
            align_language: "eng".to_string(),
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
        app
    }

    fn palette(&self) -> Palette {
        Palette {
            background: cdg_from_color32(self.color_bg),
            male_unsung: cdg_from_color32(self.color_male_unsung),
            male_highlight: cdg_from_color32(self.color_male_highlight),
            female_unsung: cdg_from_color32(self.color_female_unsung),
            female_highlight: cdg_from_color32(self.color_female_highlight),
            duet_unsung: cdg_from_color32(self.color_duet_unsung),
            duet_highlight: cdg_from_color32(self.color_duet_highlight),
            preview: cdg_from_color32(self.color_preview),
            title: cdg_from_color32(self.color_title),
            artist: cdg_from_color32(self.color_artist),
            screaming_unsung: cdg_from_color32(self.color_screaming_unsung),
            screaming_highlight: cdg_from_color32(self.color_screaming_highlight),
        }
    }

    fn video_palette(&self) -> VideoPalette {
        let c = |c: egui::Color32| video::Rgb8::new(c.r(), c.g(), c.b());
        VideoPalette {
            background: c(self.color_bg),
            male_unsung: c(self.color_male_unsung),
            male_highlight: c(self.color_male_highlight),
            female_unsung: c(self.color_female_unsung),
            female_highlight: c(self.color_female_highlight),
            duet_unsung: c(self.color_duet_unsung),
            duet_highlight: c(self.color_duet_highlight),
            preview: c(self.color_preview),
            title: c(self.color_title),
            artist: c(self.color_artist),
            screaming_unsung: c(self.color_screaming_unsung),
            screaming_highlight: c(self.color_screaming_highlight),
        }
    }

    fn singer_colors(&self, s: Singer) -> (egui::Color32, egui::Color32) {
        match s.render_as() {
            Singer::Male => (self.color_male_unsung, self.color_male_highlight),
            Singer::Female => (self.color_female_unsung, self.color_female_highlight),
            Singer::Duet => (self.color_duet_unsung, self.color_duet_highlight),
            Singer::Screaming => (self.color_screaming_unsung, self.color_screaming_highlight),
            Singer::Default => unreachable!("render_as() never returns Default"),
        }
    }

    /// Sets every color at once from a named preset - see [`ColorPreset`].
    fn apply_color_preset(&mut self, preset: ColorPreset) {
        let p = preset.palette();
        self.color_bg = color32_from_cdg(p.background);
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
        self.color_preset = preset;
    }

    /// Resolve current lyric timing into a `(timed_lines, total_duration)`
    /// pair. Used by both export and the live preview so they always agree.
    fn resolved(&self) -> (Vec<TimedLine>, f64) {
        let duration_hint = self.audio.as_ref().and_then(|a| a.duration());
        let timed = resolve_timing(&self.lines, duration_hint);
        let total = duration_hint
            .unwrap_or(0.0)
            .max(timed.last().map(|t| t.end).unwrap_or(0.0));
        (timed, total)
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
            },
            video_resolution: self.video_resolution,
            background: self.background.clone(),
            background_fit: self.background_fit,
            background_dim: self.background_dim,
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
        self.lines = project.lines;
        self.title = project.title;
        self.artist = project.artist;
        self.video_resolution = project.video_resolution;
        self.background = project.background;
        self.background_fit = project.background_fit;
        self.background_dim = project.background_dim;
        self.background_preview = None;

        let c = project.colors;
        self.color_bg = color32_from_rgb_color(c.background);
        self.color_male_unsung = color32_from_rgb_color(c.male_unsung);
        self.color_male_highlight = color32_from_rgb_color(c.male_highlight);
        self.color_female_unsung = color32_from_rgb_color(c.female_unsung);
        self.color_female_highlight = color32_from_rgb_color(c.female_highlight);
        self.color_duet_unsung = color32_from_rgb_color(c.duet_unsung);
        self.color_duet_highlight = color32_from_rgb_color(c.duet_highlight);
        self.color_preview = color32_from_rgb_color(c.preview);
        self.color_title = color32_from_rgb_color(c.title);
        self.color_artist = color32_from_rgb_color(c.artist);
        self.color_screaming_unsung = color32_from_rgb_color(c.screaming_unsung);
        self.color_screaming_highlight = color32_from_rgb_color(c.screaming_highlight);
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
            self.load_project_file(path);
        }
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
                            self.load_project_file(path);
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
    /// doesn't match `self.background` anymore). A no-op most frames -
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
        if matches!(&self.background_preview, Some((cached, cached_fit, _)) if *cached == bg && *cached_fit == fit)
        {
            return;
        }
        let pad_color = video::Rgb8::new(self.color_bg.r(), self.color_bg.g(), self.color_bg.b());
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
                self.background_preview = Some((bg, fit, Some(texture)));
            }
            Err(e) => {
                self.status = format!("Couldn't load a background preview: {e:#}");
                self.background_preview = Some((bg, fit, None));
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
            let words: Vec<String> = self.lines[orig_idx]
                .words()
                .into_iter()
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

        let language = self.align_language.trim();
        let language = if language.is_empty() { "eng" } else { language }.to_string();
        let total = requests.len();

        let progress = Arc::new(AtomicU32::new(0));
        let result: Arc<Mutex<Option<AlignJobResult>>> = Arc::new(Mutex::new(None));
        let progress_clone = progress.clone();
        let result_clone = result.clone();

        self.status = format!(
            "Auto-aligning {total} line(s)… this shells out to aeneas once per line and can \
             take a while."
        );
        self.align_job = Some(AlignJob { progress, result });

        std::thread::spawn(move || {
            if let Err(e) = align::check_aligner_available() {
                *result_clone.lock().unwrap() = Some(Err(e.to_string()));
                return;
            }
            let mut outcomes = Vec::with_capacity(total);
            for (i, (line_idx, words, window_start, window_end)) in requests.into_iter().enumerate()
            {
                let word_refs: Vec<&str> = words.iter().map(|s| s.as_str()).collect();
                let outcome =
                    align::align_line(&audio_path, &word_refs, window_start, window_end, &language)
                        .map_err(|e| e.to_string());
                outcomes.push(AlignOutcome {
                    line_idx,
                    result: outcome,
                });
                progress_clone.store(
                    (((i + 1) as f32 / total as f32) * 1000.0) as u32,
                    Ordering::Relaxed,
                );
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
                                for (i, w) in words.iter().enumerate() {
                                    if let Some(slot) = line.word_overrides.get_mut(i) {
                                        *slot = Some(w.start);
                                    }
                                    if let Some(slot) = line.word_end_overrides.get_mut(i) {
                                        *slot = Some(w.end);
                                    }
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

    fn parse_lyrics(&mut self) {
        self.lines = parse_pasted_lyrics(&self.lyrics_raw);
        self.next_untimed = 0;
        self.tap_phase = TapPhase::Start;
        self.start_edit = None;
        self.end_edit = None;
        self.status = format!(
            "Parsed {} line(s). Play the song and tap along.",
            self.lines.len()
        );
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
                        self.load_project_file(path);
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
        let Some(audio) = &self.audio else { return };
        if !audio.is_playing() {
            self.status = "Press Play first, then tap along with the song.".to_string();
            return;
        }
        if self.next_untimed >= self.lines.len() {
            self.status = "All lines are already timed.".to_string();
            return;
        }
        let pos = audio.position();
        let idx = self.next_untimed;
        match self.tap_phase {
            TapPhase::Start => {
                if let Err(e) = lyrics::check_start_change(&self.lines, idx, pos) {
                    self.status = e;
                    return;
                }
                self.lines[idx].start = Some(pos);
                self.tap_phase = TapPhase::End;
                self.status = format!(
                    "Line {} started at {} - tap again for its end.",
                    idx + 1,
                    lyrics::format_timecode(pos)
                );
            }
            TapPhase::End => {
                if let Err(e) = lyrics::check_end_change(&self.lines, idx, pos) {
                    self.status = e;
                    return;
                }
                self.lines[idx].sing_end_override = Some(pos);
                self.tap_phase = TapPhase::Start;
                self.next_untimed += 1;

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
    /// yet, resume if paused, otherwise pause - the same three actions as
    /// the Play/Pause/Resume buttons, just combined into whichever one
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
        self.next_untimed = self
            .lines
            .iter()
            .position(|l| l.start.is_none() || l.sing_end_override.is_none())
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

    /// Same as [`Self::resolved`], but also returns each `TimedLine`'s index
    /// into `self.lines` (needed so the fine-tune panel can edit the right
    /// original line, since `resolved()` sorts by start time).
    fn resolved_with_indices(&self) -> (Vec<TimedLine>, Vec<usize>) {
        let duration_hint = self.audio.as_ref().and_then(|a| a.duration());
        let mut sorted: Vec<(usize, f64)> = self
            .lines
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.start.map(|s| (i, s)))
            .collect();
        sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        let mut timed = Vec::with_capacity(sorted.len());
        let mut indices = Vec::with_capacity(sorted.len());
        for pos in 0..sorted.len() {
            let (orig_idx, start) = sorted[pos];
            let end = if pos + 1 < sorted.len() {
                sorted[pos + 1].1
            } else {
                match duration_hint {
                    Some(d) if d > start => d,
                    _ => start + 4.0,
                }
            };
            let line = &self.lines[orig_idx];
            timed.push(TimedLine::from_line(line, start, end));
            indices.push(orig_idx);
        }
        (timed, indices)
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
        let pos = audio.position();
        if let Err(e) = lyrics::check_start_change(&self.lines, idx, pos) {
            self.status = e;
            return;
        }
        self.lines[idx].start = Some(pos);
        self.recompute_next_untimed();
    }

    fn reset_timing(&mut self) {
        for l in &mut self.lines {
            l.start = None;
            l.sing_end_override = None;
        }
        self.next_untimed = 0;
        self.tap_phase = TapPhase::Start;
        self.status = "Timing cleared.".to_string();
    }

    fn tap_word_start(&mut self, line_idx: usize, word_idx: usize) {
        let Some(audio) = &self.audio else { return };
        if !audio.is_playing() {
            self.status = "Press Play first, then click a word to set its time.".to_string();
            return;
        }
        let pos = audio.position();
        if let Some(line) = self.lines.get_mut(line_idx) {
            if let Some(slot) = line.word_overrides.get_mut(word_idx) {
                *slot = Some(pos);
            }
        }
    }

    /// Tap the moment a word's held-out highlight should stop advancing -
    /// lets a word's color-wipe finish (and freeze) *before* the next
    /// word's start, instead of always stretching to fill that whole gap.
    /// See [`lyrics::LyricLine::word_end_overrides`].
    fn tap_word_end(&mut self, line_idx: usize, word_idx: usize) {
        let Some(audio) = &self.audio else { return };
        if !audio.is_playing() {
            self.status = "Press Play first, then click a word to set its end time.".to_string();
            return;
        }
        let pos = audio.position();
        if let Some(line) = self.lines.get_mut(line_idx) {
            if let Some(slot) = line.word_end_overrides.get_mut(word_idx) {
                *slot = Some(pos);
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
        let pos = audio.position();
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
            self.export_base_name = self
                .audio
                .as_ref()
                .and_then(|a| a.file_name())
                .map(|n| n.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or(n))
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    let t = self.title.trim();
                    (!t.is_empty()).then(|| t.to_string())
                })
                .unwrap_or_else(|| "karaoke".to_string());
        }
        self.show_export_dialog = true;
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
        if !(self.export_cdg || self.export_video || self.export_lrc || self.export_ultrastar) {
            return None;
        }
        let total = self.lines.len();
        let timed_count = self.lines.iter().filter(|l| l.start.is_some()).count();
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
                        ui.checkbox(&mut self.remove_vocals_for_video, "Remove vocals (slow)");
                    });
                });

                ui.checkbox(&mut self.export_instrumental, "Instrumental audio");
                ui.add_enabled_ui(self.export_instrumental, |ui| {
                    ui.indent("instrumental_opts", |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Format:");
                            egui::ComboBox::from_id_source("export_instrumental_format")
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

                if self.export_video || self.export_instrumental {
                    ui.label(
                        egui::RichText::new(
                            "Vocal removal runs a real ML separation model (UVR-MDX-NET, via \
                             the audio-separator command-line tool) - needs audio-separator \
                             installed separately (pip install audio-separator) and can take \
                             a while, especially without a GPU.",
                        )
                        .small()
                        .weak(),
                    );
                }
                if self.export_ultrastar {
                    ui.label(
                        egui::RichText::new(
                            "UltraStar export has no pitch data (this app doesn't track \
                             melody) - every note is written at a constant placeholder pitch, \
                             so a game that scores pitch accuracy will treat the whole song as \
                             one fixed note. Lyrics and timing are real.",
                        )
                        .small()
                        .weak(),
                    );
                }
                if self.export_lrc || self.export_ultrastar {
                    if let Some(audio) = &self.audio {
                        if audio.is_loaded() {
                            ui.label(
                                egui::RichText::new(
                                    "The loaded audio is copied alongside lyrics exports too \
                                     (same base filename), same as the .cdg pairing.",
                                )
                                .small()
                                .weak(),
                            );
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
                        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
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

                ui.add_space(10.0);
                let can_export = (self.export_cdg
                    || self.export_lrc
                    || self.export_ultrastar
                    || self.export_video
                    || self.export_instrumental)
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
        let do_cdg = self.export_cdg;
        let do_lrc = self.export_lrc;
        let do_ultrastar = self.export_ultrastar;
        let do_video = self.export_video;
        let do_instrumental = self.export_instrumental;
        if !do_cdg && !do_lrc && !do_ultrastar && !do_video && !do_instrumental {
            self.status = "Choose at least one output to export.".to_string();
            return;
        }
        let Some(folder) = self.export_folder.clone() else {
            self.status = "Choose an output folder first.".to_string();
            return;
        };
        let base = self.export_base_name.trim();
        let base = if base.is_empty() { "karaoke" } else { base }.to_string();

        let audio_path = self
            .audio
            .as_ref()
            .and_then(|a| a.path())
            .map(|p| p.to_path_buf());
        if (do_video || do_instrumental) && audio_path.is_none() {
            self.status =
                "Load an audio file first - video/instrumental export need it.".to_string();
            return;
        }

        let (timed, total_duration) = self.resolved();
        let cdg_palette = self.palette();
        let video_palette = self.video_palette();
        let title = (!self.title.trim().is_empty()).then(|| self.title.trim().to_string());
        let artist = (!self.artist.trim().is_empty()).then(|| self.artist.trim().to_string());
        let resolution = self.video_resolution;
        let background = self.background.clone();
        let background_fit = self.background_fit;
        let background_dim = self.background_dim;
        let remove_vocals_for_video = self.remove_vocals_for_video;
        let instrumental_ext = self.instrumental_format.extension();
        let lrc_enhanced = self.lrc_enhanced_words;

        let cdg_path = folder.join(format!("{base}.cdg"));
        let lrc_path = folder.join(format!("{base}.lrc"));
        let ultrastar_path = folder.join(format!("{base}.txt"));
        let video_path = folder.join(format!("{base}.mp4"));
        let instrumental_path = folder.join(format!("{base}-instrumental.{instrumental_ext}"));

        let progress = Arc::new(AtomicU32::new(0));
        let result: Arc<Mutex<Option<Vec<ExportOutcome>>>> = Arc::new(Mutex::new(None));
        let progress_clone = progress.clone();
        let result_clone = result.clone();

        self.status = "Exporting…".to_string();
        self.show_export_dialog = false;
        self.combined_export = Some(CombinedExportHandle { progress, result });

        std::thread::spawn(move || {
            let mut outcomes = Vec::new();

            // Equal-weighted phases across whatever was selected - video
            // reports its own fine-grained sub-progress within its slice
            // (usually the slowest step by far), the others just jump from
            // 0% to 100% of their slice on completion.
            let phase_count = do_cdg as u32
                + do_lrc as u32
                + do_ultrastar as u32
                + do_video as u32
                + do_instrumental as u32;
            let phase_count = phase_count.max(1);
            let mut phase_index = 0u32;
            let set_progress = move |idx: u32, frac_within: f32| {
                let overall = (idx as f32 + frac_within.clamp(0.0, 1.0)) / phase_count as f32;
                progress_clone.store((overall * 1000.0) as u32, Ordering::Relaxed);
            };

            // Auto-pair the loaded audio next to whichever of .cdg/.lrc/
            // UltraStar's .txt actually gets written (same base filename,
            // in the same folder) so it's ready for pickup by a karaoke
            // player/game without a manual copy or rename - all of these
            // resolve to the *same* paired-audio destination for a given
            // base name/folder, so this only needs to run once even if
            // several of them are selected together.
            let mut paired_audio_sibling: Option<PathBuf> = None;

            if do_cdg {
                let bytes = export::render_cdg(
                    &timed,
                    total_duration,
                    &cdg_palette,
                    title.as_deref(),
                    artist.as_deref(),
                );
                let r = std::fs::write(&cdg_path, &bytes)
                    .map(|()| cdg_path.clone())
                    .map_err(|e| e.to_string());
                if r.is_ok() {
                    paired_audio_sibling.get_or_insert_with(|| cdg_path.clone());
                }
                outcomes.push(ExportOutcome {
                    label: "CDG file",
                    result: r,
                });

                phase_index += 1;
                set_progress(phase_index, 0.0);
            }

            if do_lrc {
                let text =
                    formats::export_lrc(&timed, lrc_enhanced, title.as_deref(), artist.as_deref());
                let r = std::fs::write(&lrc_path, text)
                    .map(|()| lrc_path.clone())
                    .map_err(|e| e.to_string());
                if r.is_ok() {
                    paired_audio_sibling.get_or_insert_with(|| lrc_path.clone());
                }
                outcomes.push(ExportOutcome {
                    label: "LRC lyrics",
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
                    label: "UltraStar lyrics",
                    result: r,
                });

                phase_index += 1;
                set_progress(phase_index, 0.0);
            }

            if let Some(sibling) = &paired_audio_sibling {
                if let Some(audio_path) = &audio_path {
                    let paired =
                        export::copy_paired_audio(audio_path, sibling).map_err(|e| e.to_string());
                    outcomes.push(ExportOutcome {
                        label: "Paired audio",
                        result: paired,
                    });
                }
            }

            // Vocal separation is shared: if both the instrumental export
            // and the video's "remove vocals" are requested, run the
            // (slow) separation model once and reuse it for both, instead
            // of separating the same song twice.
            let mut shared_instrumental: Option<PathBuf> = None;

            if do_instrumental {
                if let Some(audio_path) = &audio_path {
                    let r = vocals::remove_vocals_to_file(audio_path, &instrumental_path);
                    if r.is_ok() {
                        shared_instrumental = Some(instrumental_path.clone());
                    }
                    outcomes.push(ExportOutcome {
                        label: "Instrumental audio",
                        result: r
                            .map(|()| instrumental_path.clone())
                            .map_err(|e| e.to_string()),
                    });
                }
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
                        if let Some(shared) = &shared_instrumental {
                            shared.clone()
                        } else {
                            let tmp = std::env::temp_dir().join(format!(
                                "abyssal-cdg-instrumental-{}.wav",
                                std::process::id()
                            ));
                            vocals::remove_vocals_to_file(audio_path, &tmp)
                                .map_err(|e| format!("Vocal removal failed: {e}"))?;
                            own_temp = Some(tmp.clone());
                            tmp
                        }
                    } else {
                        audio_path.clone()
                    };

                    let video_phase = phase_index;
                    let r = video::render_video(
                        &timed,
                        total_duration,
                        &video_palette,
                        title.as_deref(),
                        artist.as_deref(),
                        resolution,
                        30,
                        &render_audio_path,
                        background.as_ref(),
                        background_fit,
                        background_dim,
                        &video_path,
                        |p| set_progress(video_phase, p),
                    );

                    if let Some(tmp) = &own_temp {
                        let _ = std::fs::remove_file(tmp);
                    }

                    r.map(|()| video_path.clone()).map_err(|e| e.to_string())
                })();

                outcomes.push(ExportOutcome {
                    label: "Video",
                    result: video_result,
                });
                phase_index += 1;
                set_progress(phase_index, 0.0);
            }

            *result_clone.lock().unwrap() = Some(outcomes);
        });
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

    /// Draws the live "what will this look like" preview: a mock TV screen
    /// showing exactly what the exported CDG will show at the current
    /// playback position (or position 0 if nothing is playing). When a
    /// background image/video is set, it's drawn (dimmed) behind everything
    /// else here too, so legibility can be judged without exporting first -
    /// though a background video shows a single representative frame here
    /// rather than true playback (see [`video::extract_video_background_thumbnail`]).
    fn draw_preview(&mut self, ui: &mut egui::Ui) {
        let t = self.audio.as_ref().map(|a| a.position()).unwrap_or(0.0);
        let (timed, _total) = self.resolved();
        let card_end = export::title_card_end(&timed);

        let width = ui.available_width().min(480.0);
        let height = width * 9.0 / 16.0;

        // A fixed, modest resolution for the preview background - it's
        // scaled up to whatever `width`x`height` ends up being when drawn,
        // and doesn't need to match the export's real resolution.
        const PREVIEW_BG_SIZE: (usize, usize) = (480, 270);
        self.ensure_background_preview(ui.ctx(), PREVIEW_BG_SIZE.0, PREVIEW_BG_SIZE.1);

        let (rect, _response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        match self
            .background_preview
            .as_ref()
            .and_then(|(_, _, t)| t.as_ref())
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
        let ui = &mut content_ui;
        {
            let legend_singers = singer_legend(&timed);
            let has_title_card = !self.title.trim().is_empty()
                || !self.artist.trim().is_empty()
                || !legend_singers.is_empty();

            ui.vertical_centered(|ui| {
                ui.add_space(height * 0.15);

                if has_title_card && t < card_end {
                    if !self.title.trim().is_empty() {
                        ui.colored_label(
                            self.color_title,
                            egui::RichText::new(&self.title).size(20.0).strong(),
                        );
                    }
                    if !self.artist.trim().is_empty() {
                        ui.colored_label(
                            self.color_artist,
                            egui::RichText::new(format!("by {}", self.artist)).size(14.0),
                        );
                    }
                    if !legend_singers.is_empty() {
                        ui.add_space(6.0);
                        // Built as a single multi-colored label (rather than a
                        // `ui.horizontal` of separate labels) so
                        // `vertical_centered` centers the whole legend as one
                        // block, the same way it centers the title/artist text.
                        let mut job = egui::text::LayoutJob::default();
                        for (i, singer) in legend_singers.iter().enumerate() {
                            if i > 0 {
                                job.append("   ", 0.0, egui::TextFormat::default());
                            }
                            let (_, highlight) = self.singer_colors(*singer);
                            job.append(
                                singer.label(),
                                0.0,
                                egui::TextFormat {
                                    color: highlight,
                                    font_id: egui::FontId::proportional(12.0),
                                    ..Default::default()
                                },
                            );
                        }
                        ui.label(job);
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
                        if let Some((cd_start, cd_end)) =
                            lyrics::countdown_window_between(card_end, first.start)
                        {
                            if t >= cd_start {
                                let frac = ((t - cd_start) / (cd_end - cd_start).max(0.001))
                                    .clamp(0.0, 1.0);
                                let lit = ((frac * 4.0).floor() as i32 + 1).clamp(0, 4) as usize;
                                let (_, highlight) = self.singer_colors(first.singer);
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    ui.add_space(width / 2.0 - 40.0);
                                    for dot in 0..4 {
                                        let color = if dot < lit {
                                            highlight
                                        } else {
                                            self.color_preview
                                        };
                                        let (rect, _) = ui.allocate_exact_size(
                                            egui::vec2(14.0, 14.0),
                                            egui::Sense::hover(),
                                        );
                                        ui.painter().circle_filled(rect.center(), 6.0, color);
                                        ui.add_space(8.0);
                                    }
                                });
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
                        // should read as "done, waiting" (then the countdown, then the
                        // next line), not show lyrics that are still a break away.
                        let current_line = &timed[current_idx];
                        let hide_upcoming = hide_upcoming_lines(current_line, t);
                        let blank_sung = blank_sung_lines(current_line, t);

                        for (slot, &idx) in block.iter().enumerate() {
                            let line = &timed[idx];
                            let (unsung, highlight) = self.singer_colors(line.singer);
                            let normalized = lyrics::normalize_text(&line.text);
                            match slot.cmp(&slot_in_block) {
                                std::cmp::Ordering::Less => {
                                    // Already sung - shown fully in the highlight color.
                                    if blank_sung {
                                        ui.add_space(22.0);
                                    } else {
                                        ui.colored_label(
                                            highlight,
                                            egui::RichText::new(normalized).size(18.0),
                                        );
                                    }
                                }
                                std::cmp::Ordering::Equal => {
                                    if blank_sung {
                                        ui.add_space(22.0);
                                        continue;
                                    }
                                    let mut job = egui::text::LayoutJob {
                                        halign: egui::Align::Center,
                                        ..Default::default()
                                    };
                                    // Same continuous per-line fraction the video/CDG exporters
                                    // use, so the wipe moves smoothly through a word's letters
                                    // as it's held out instead of the whole word snapping to
                                    // `highlight` the instant its timestamp is reached (which
                                    // looked instantaneous for long-held words and words with
                                    // few characters alike).
                                    let chars: Vec<char> = normalized.chars().collect();
                                    let spans = lyrics::word_char_spans(&normalized);
                                    let boundary = lyrics::current_line_wipe_fraction(line, t)
                                        * chars.len().max(1) as f32;
                                    let font_id = egui::FontId::proportional(18.0);
                                    for (i, &(offset, len)) in spans.iter().enumerate() {
                                        let word_text: String =
                                            chars[offset..offset + len].iter().collect();
                                        let split = ((boundary - offset as f32)
                                            .round()
                                            .clamp(0.0, len as f32))
                                            as usize;
                                        let sung_part: String =
                                            word_text.chars().take(split).collect();
                                        let rest: String = word_text.chars().skip(split).collect();
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
                                }
                                std::cmp::Ordering::Greater => {
                                    if hide_upcoming {
                                        ui.add_space(22.0);
                                    } else {
                                        ui.colored_label(
                                            unsung,
                                            egui::RichText::new(normalized).size(18.0),
                                        );
                                    }
                                }
                            }
                        }

                        ui.add_space(8.0);

                        if let Some((cd_start, cd_end)) = countdown_window(current_line) {
                            if t >= cd_start {
                                let frac = ((t - cd_start) / (cd_end - cd_start).max(0.001))
                                    .clamp(0.0, 1.0);
                                let lit = ((frac * 4.0).floor() as i32 + 1).clamp(0, 4) as usize;
                                let next_singer = timed
                                    .get(current_idx + 1)
                                    .map(|l| l.singer)
                                    .unwrap_or(current_line.singer);
                                let (_, next_highlight) = self.singer_colors(next_singer);
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    ui.add_space(width / 2.0 - 40.0);
                                    for dot in 0..4 {
                                        let color = if dot < lit {
                                            next_highlight
                                        } else {
                                            self.color_preview
                                        };
                                        let (rect, _) = ui.allocate_exact_size(
                                            egui::vec2(14.0, 14.0),
                                            egui::Sense::hover(),
                                        );
                                        ui.painter().circle_filled(rect.center(), 6.0, color);
                                        ui.add_space(8.0);
                                    }
                                });
                            }
                        }
                    }
                    None => {
                        ui.label(
                            egui::RichText::new("♪")
                                .size(28.0)
                                .color(self.color_preview),
                        );
                    }
                }
            });
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
        let (timed, indices) = self.resolved_with_indices();
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
                let last_end = timed.last().map(|t| t.end).unwrap_or(30.0).max(1.0);
                self.timeline_view.px_per_sec = (area_width / last_end as f32)
                    .clamp(timeline::MIN_PX_PER_SEC, timeline::MAX_PX_PER_SEC);
                self.timeline_view.scroll_secs = 0.0;
            }
            ui.label(
                "Drag a bubble to move it, its edges to trim start/end. Click or drag \
                 empty space to seek/scrub playback - scroll to zoom, shift+scroll to pan.",
            );
            if self.waveform_job.is_some() {
                ui.label(egui::RichText::new("(building waveform…)").weak());
            }
        });

        if timed.is_empty() {
            ui.label(
                "Time at least one line (tap along, or use the table below) to use the timeline.",
            );
            return;
        }

        let ruler_h = 16.0;
        let line_row_h = 28.0;
        let row_gap = 6.0;
        let word_row_h = 28.0;
        let height = ruler_h + 4.0 + line_row_h + row_gap + word_row_h + 4.0;
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
        let line_row_top = rect.top() + ruler_h + 4.0;
        if let Some(waveform) = &self.waveform {
            let wave_top = line_row_top;
            let wave_bottom = line_row_top + line_row_h + row_gap + word_row_h;
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

        // Line bubbles - one per timed line, in start-time order.
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
            if let Some(icon) = timeline_cursor_icon(active_mode, &bubble_response, bubble_rect) {
                ui.ctx().set_cursor_icon(icon);
            }

            if bubble_response.clicked() {
                self.word_tap_line = Some(orig_idx);
            }

            if bubble_response.drag_started() {
                if let Some(pos) = bubble_response.interact_pointer_pos() {
                    let local_x = pos.x - bubble_rect.left();
                    let mode = timeline::classify_drag(local_x, bubble_rect.width());
                    let prev_sing_end = if i > 0 {
                        Some(timed[i - 1].sing_end)
                    } else {
                        None
                    };
                    let next_start = timed.get(i + 1).map(|t| t.start);
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
                            pos.x,
                        ),
                    });
                }
            }

            if bubble_response.dragged() {
                if let Some(pos) = bubble_response.interact_pointer_pos() {
                    let resolved = self
                        .timeline_drag
                        .as_ref()
                        .filter(|d| d.line_idx == orig_idx && d.target == TimelineDragTarget::Line)
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
            let prev_line_sing_end = if i > 0 {
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
                let bubble_response = ui.interact(bubble_rect, id, egui::Sense::click_and_drag());

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
                if let Some(icon) = timeline_cursor_icon(active_mode, &bubble_response, bubble_rect)
                {
                    ui.ctx().set_cursor_icon(icon);
                }

                if bubble_response.drag_started() {
                    if let Some(pos) = bubble_response.interact_pointer_pos() {
                        let local_x = pos.x - bubble_rect.left();
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
                        let max_sing_end = if w == last_word_idx {
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
                                pos.x,
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
                                    // The first word pushing earlier than
                                    // the line's own start pulls the
                                    // line's start along with it.
                                    if w == 0 && s < l.start.unwrap_or(s) {
                                        l.start = Some(s);
                                    }
                                }
                                if let Some(e) = new_end {
                                    if let Some(slot) = l.word_end_overrides.get_mut(w) {
                                        *slot = Some(e);
                                    }
                                    // Symmetrically, the last word
                                    // pushing later than the line's own
                                    // sing_end extends the line to match.
                                    if w == last_word_idx && e > line.sing_end {
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
                            timeline::DragMode::Body | timeline::DragMode::LeftEdge => self.lines
                                [orig_idx]
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
        self.auto_follow_word_tap_line();

        // Ctrl+S / Ctrl+Shift+S save the project - unlike the shortcuts
        // below, these are safe to fire even while a text field has focus,
        // the same way most apps let Ctrl+S through while you're typing.
        if ctx.input(|i| i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::S)) {
            self.save_project();
        }
        if ctx.input(|i| i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::S)) {
            self.save_project_as_dialog();
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
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("Abyssal CDG Creator");
            });
            ui.add_space(4.0);

            if let Some(err) = &self.audio_error {
                ui.colored_label(egui::Color32::RED, err);
            }

            ui.horizontal(|ui| {
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
            ui.label(
                egui::RichText::new(
                    format!(
                        "Saves everything - lyrics, timing, colors, the audio file's path - \
                         to a .{} project file, so you can pick up where you left off. \
                         Ctrl+S saves; Ctrl+Shift+S always asks for a location. Ctrl+Z undoes \
                         a tap/drag/nudge/edit; Ctrl+Shift+Z (or Ctrl+Y) redoes it.",
                        project::FILE_EXTENSION
                    ),
                )
                .small()
                .weak(),
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

            ui.horizontal(|ui| {
                let has_audio = self.audio.as_ref().map(|a| a.is_loaded()).unwrap_or(false);
                ui.add_enabled_ui(has_audio, |ui| {
                    if ui.button("▶ Play from start").clicked() {
                        if let Some(a) = &mut self.audio {
                            if let Err(e) = a.play_from_start() {
                                self.status = format!("Playback error: {e}");
                            }
                        }
                    }
                    if ui.button("⏸ Pause").clicked() {
                        if let Some(a) = &mut self.audio {
                            a.pause();
                        }
                    }
                    if ui.button("▶ Resume").clicked() {
                        if let Some(a) = &mut self.audio {
                            a.resume();
                        }
                    }
                    if ui.button("⏹ Stop").clicked() {
                        if let Some(a) = &mut self.audio {
                            a.stop();
                        }
                    }
                });
                if let Some(a) = &self.audio {
                    ui.label(format!("{} / {}", format_time(a.position()), format_time(a.duration().unwrap_or(0.0))));
                }
            });

            ui.horizontal(|ui| {
                let known_duration = self.audio.as_ref().and_then(|a| a.duration());
                let has_audio = self.audio.as_ref().map(|a| a.is_loaded()).unwrap_or(false);
                ui.add_enabled_ui(has_audio, |ui| {
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
                });
                if known_duration.is_none() && has_audio {
                    ui.label("(exact song length unknown - seek range is approximate)");
                }
            });
            ui.label("Drag the bar (or use ⏪/⏩, or the left/right arrow keys) to jump anywhere in the song - handy for redoing a line without replaying from the start.");

            ui.horizontal(|ui| {
                ui.label("Song title:");
                ui.add(egui::TextEdit::singleline(&mut self.title).desired_width(220.0));
                ui.label("Artist:");
                ui.add(egui::TextEdit::singleline(&mut self.artist).desired_width(220.0));
            });
            ui.label(
                egui::RichText::new(
                    "Tip: name imported audio files \"Artist - Song Name.ext\" and these \
                     fields fill in automatically when they're both still blank.",
                )
                .small()
                .weak(),
            );
        });

        self.draw_export_dialog(ctx);

        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| {
            ui.add_space(4.0);
            let busy = self.project_busy();
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if ui
                        .add_sized([120.0, 28.0], egui::Button::new("Export…"))
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
                        .selected_text(self.align_language.clone())
                        .show_ui(ui, |ui| {
                            for (code, label) in ALIGN_LANGUAGES {
                                ui.selectable_value(
                                    &mut self.align_language,
                                    code.to_string(),
                                    format!("{label} ({code})"),
                                );
                            }
                        });
                });
                if busy {
                    ui.spinner();
                }
            });
            ui.label(
                egui::RichText::new(
                    "Auto-align uses forced alignment (aeneas, run once per already-timed \
                     line) to fill in every word's timing automatically, replacing any \
                     existing word timing (estimated or manually tapped) for lines it \
                     successfully aligns. Needs aeneas installed separately \
                     (pip install aeneas - see the README). Undo (Ctrl+Z) if a result \
                     doesn't look right.",
                )
                .small()
                .weak(),
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
                ui.label(&self.status);
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
                        ui.add(
                            egui::TextEdit::multiline(&mut self.lyrics_raw)
                                .desired_rows(14)
                                .desired_width(f32::INFINITY),
                        );
                    });
                if ui.button("Parse lyrics").clicked() {
                    self.parse_lyrics();
                }
                if ui.button("Load lyrics file…").clicked() {
                    self.load_lyrics_file_dialog();
                }
                ui.label(
                    egui::RichText::new(
                        "Supports LRC (.lrc), UltraStar (.txt), and KOK - auto-detected.",
                    )
                    .small()
                    .weak(),
                );
                ui.add_space(8.0);
                ui.separator();
                ui.label("2. Tap along:");
                ui.label(
                    "Press Play, then press Space (or click below) the instant each line \
                     starts being sung, and again the instant it ends - two taps per line. \
                     Lines fill in top to bottom automatically - missed one? Drag the seek \
                     bar back a few seconds and try again. (Skipping the end-tap is fine too - \
                     it'll just fall back to an automatic estimate until you set it, here or \
                     in the table below.) Press M/F/D/S to set that line's voice (Male/Female/\
                     Duet/Screaming) without leaving the keyboard.",
                );
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
                        .add_sized([ui.available_width(), 48.0], egui::Button::new(label))
                        .clicked()
                    {
                        self.tap_next();
                    }
                });
            });

        egui::SidePanel::right("colors_preview_panel")
            .resizable(true)
            .default_width(340.0)
            .width_range(260.0..=560.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_source("right_panel_scroll")
                    .show(ui, |ui| {
                        ui.label("4. Preview (matches the exported file):");
                        self.draw_preview(ui);
                        ui.add_space(10.0);
                        ui.separator();
                        egui::CollapsingHeader::new("Colors")
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
                                ui.label(
                                    egui::RichText::new(
                                        "Pick a look, or fine-tune individual colors below.",
                                    )
                                    .small()
                                    .weak(),
                                );
                                ui.add_space(6.0);
                                egui::Grid::new("colors_grid")
                                    .num_columns(2)
                                    .spacing([8.0, 4.0])
                                    .show(ui, |ui| {
                                        ui.label("Background");
                                        ui.color_edit_button_srgba(&mut self.color_bg);
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
                                ui.label(
                                    egui::RichText::new(
                                        "Show an image or video behind the lyrics in the video \
                                         export and the preview above, instead of the flat \
                                         Background color. The .cdg export always uses the flat \
                                         color instead - it's a fixed 300x216, 16-color format \
                                         with no room for real images.",
                                    )
                                    .small()
                                    .weak(),
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
                                ui.label(
                                    egui::RichText::new(
                                        "Cover crops the edges so nothing gets stretched; \
                                         Contain shows the whole image/video with bars added \
                                         if the aspect ratio doesn't match.",
                                    )
                                    .small()
                                    .weak(),
                                );
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    ui.label("Dim:");
                                    ui.add(
                                        egui::Slider::new(&mut self.background_dim, 0.0..=1.0)
                                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                                    );
                                });
                                ui.label(
                                    egui::RichText::new(
                                        "How much of a black scrim to blend over the \
                                         background image/video so the lyrics stay legible on \
                                         top of it.",
                                    )
                                    .small()
                                    .weak(),
                                );
                            });
                    });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("3. Review / fine-tune timestamps and voice:");

            // Per-word fine-tuning strip - only shown while a line is selected.
            if let Some(i) = self.word_tap_line {
                if i < self.lines.len() {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.strong("Fine-tune words:");
                            ui.label(&self.lines[i].text);
                            ui.checkbox(&mut self.auto_follow_words, "Auto-follow");
                            if ui.small_button("Close").clicked() {
                                self.word_tap_line = None;
                            }
                        });
                        ui.label(if self.auto_follow_words {
                            "Play the song and just click each word the instant it's sung - \
                             this panel follows along automatically as the song moves from \
                             line to line, so there's no need to reselect a line yourself. \
                             Click a word again to retime it. If auto-follow keeps snatching \
                             the panel away before you can catch a line's first or last word, \
                             turn it off."
                        } else {
                            "Auto-follow is off - this panel stays on this line until you pick \
                             another one (below, or by clicking a lyric line in the timeline), \
                             so a line's first/last word is easier to catch. Click a word again \
                             to retime it."
                        });
                        ui.horizontal(|ui| {
                            ui.label("Tap sets a word's:");
                            ui.selectable_value(
                                &mut self.word_tap_mode,
                                WordTapMode::Start,
                                "Start",
                            );
                            ui.selectable_value(&mut self.word_tap_mode, WordTapMode::End, "End");
                            ui.label(
                                "- by default a word's highlight just runs until the next \
                                 word starts; set an End to make it stop (and freeze) sooner, \
                                 e.g. for a held note followed by a pause.",
                            );
                        });
                        ui.label(
                            "Green = start manually timed, blue = end manually timed, teal = \
                             both; the rest still use the automatic estimate.",
                        );
                        let words: Vec<String> = self.lines[i]
                            .text
                            .split_whitespace()
                            .map(|s| s.to_string())
                            .collect();
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

            egui::ScrollArea::both()
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
                    for (t, &orig_idx) in timed_for_grid.iter().zip(indices_for_grid.iter()) {
                        estimated_end[orig_idx] = Some(t.sing_end);
                    }

                    egui::Grid::new("lines_grid")
                        .num_columns(7)
                        .striped(true)
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            let mut retap_idx: Option<usize> = None;
                            let mut words_idx: Option<usize> = None;
                            let mut nudge: Option<(usize, f64)> = None;
                            let mut clear_idx: Option<usize> = None;
                            let mut commit_start: Option<(usize, String)> = None;
                            let mut commit_end: Option<(usize, String)> = None;

                            ui.label(egui::RichText::new("Start").small().weak());
                            ui.label(egui::RichText::new("Lyric").small().weak());
                            ui.label(egui::RichText::new("End").small().weak());
                            ui.label(egui::RichText::new("Singer").small().weak());
                            ui.label("");
                            ui.label("");
                            ui.label("");
                            ui.end_row();

                            // Not a `.zip()`/`.enumerate()` candidate: each
                            // iteration needs the plain index `i` itself (to
                            // defer mutation - see the `retap_idx`/etc.
                            // handling below, which avoids double-borrowing
                            // `self` while its own fields are being edited
                            // inline above), not just a borrowed element.
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
                                        .desired_width(78.0)
                                        .hint_text("00:00.00"),
                                );
                                if start_resp.has_focus() {
                                    self.start_edit = Some((i, start_buf));
                                } else if start_resp.lost_focus() {
                                    commit_start = Some((i, start_buf));
                                    self.start_edit = None;
                                }

                                ui.scope(|ui| {
                                    ui.set_max_width(220.0);
                                    let text_label = if is_next {
                                        egui::RichText::new(&self.lines[i].text).strong()
                                    } else {
                                        egui::RichText::new(&self.lines[i].text)
                                    };
                                    ui.add(egui::Label::new(text_label).wrap());
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
                                        .desired_width(78.0)
                                        .hint_text(end_hint),
                                );
                                if end_resp.has_focus() {
                                    self.end_edit = Some((i, end_buf));
                                } else if end_resp.lost_focus() {
                                    commit_end = Some((i, end_buf));
                                    self.end_edit = None;
                                }

                                egui::ComboBox::from_id_source(("singer", i))
                                    .width(82.0)
                                    .selected_text(self.lines[i].singer.label())
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.lines[i].singer,
                                            Singer::Default,
                                            "Default",
                                        );
                                        ui.selectable_value(
                                            &mut self.lines[i].singer,
                                            Singer::Male,
                                            "Male",
                                        );
                                        ui.selectable_value(
                                            &mut self.lines[i].singer,
                                            Singer::Female,
                                            "Female",
                                        );
                                        ui.selectable_value(
                                            &mut self.lines[i].singer,
                                            Singer::Duet,
                                            "Duet",
                                        );
                                        ui.selectable_value(
                                            &mut self.lines[i].singer,
                                            Singer::Screaming,
                                            "Screaming",
                                        );
                                    });

                                if ui.small_button("Tap").clicked() {
                                    retap_idx = Some(i);
                                }
                                if ui.small_button("Words").clicked() {
                                    words_idx = Some(i);
                                }
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
                                ui.end_row();
                            }

                            if let Some(i) = retap_idx {
                                self.retap_line(i);
                            }
                            if let Some(i) = words_idx {
                                self.word_tap_line = Some(i);
                            }
                            if let Some((i, delta)) = nudge {
                                if let Some(s) = &mut self.lines[i].start {
                                    *s = (*s + delta).max(0.0);
                                }
                                if let Some(new_start) = self.lines[i].start {
                                    self.audition_seek(new_start);
                                }
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
        });

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
}
