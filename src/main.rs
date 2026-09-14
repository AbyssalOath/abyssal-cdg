// Windows GUI apps default to the "console" subsystem, which pops up (and
// keeps open) a terminal window behind the app for the lifetime of the
// process. Switching to the "windows" subsystem in release builds hides
// that window - debug builds keep the console so `println!`/panic output
// is still visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod cdg;
mod export;
mod font;
mod formats;
mod lyrics;
mod video;

use audio::AudioPlayer;
use eframe::egui;
use export::Palette;
use lyrics::{
    countdown_window, group_into_blocks, hide_upcoming_lines, parse_pasted_lyrics, resolve_timing,
    LyricLine, Singer, TimedLine,
};
use std::path::PathBuf;
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

struct KaraokeApp {
    audio: Option<AudioPlayer>,
    audio_error: Option<String>,

    lyrics_raw: String,
    lines: Vec<LyricLine>,
    /// Index of the next line "Tap" will assign a timestamp to.
    next_untimed: usize,
    /// If set, the fine-tune-words panel is open for this line index.
    word_tap_line: Option<usize>,

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

    status: String,

    /// While the seek bar is being dragged, holds the not-yet-committed
    /// position so the displayed handle doesn't fight with the live
    /// playback position updating every frame.
    seek_drag_value: Option<f64>,

    video_resolution: Resolution,
    /// Set while a video export is running in a background thread.
    video_export: Option<VideoExportHandle>,
}

/// Shared state for a video export running on a background thread, so the
/// GUI stays responsive and can show a progress bar (encoding a full song
/// can take anywhere from several seconds to a couple of minutes).
struct VideoExportHandle {
    progress: Arc<AtomicU32>, // 0..=1000 (tenths of a percent)
    result: Arc<Mutex<Option<Result<PathBuf, String>>>>,
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
        Self {
            audio,
            audio_error,
            lyrics_raw: String::new(),
            lines: Vec::new(),
            next_untimed: 0,
            word_tap_line: None,
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
            status: String::new(),
            seek_drag_value: None,
            video_resolution: Resolution::Hd1080,
            video_export: None,
        }
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
        match s {
            Singer::Male => (self.color_male_unsung, self.color_male_highlight),
            Singer::Female => (self.color_female_unsung, self.color_female_highlight),
            Singer::Duet => (self.color_duet_unsung, self.color_duet_highlight),
            Singer::Screaming => (self.color_screaming_unsung, self.color_screaming_highlight),
        }
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
            match audio.load(path) {
                Ok(()) => {
                    self.status = "Audio loaded.".to_string();
                }
                Err(e) => {
                    self.status = format!("Couldn't load audio: {e}");
                }
            }
        }
    }

    fn parse_lyrics(&mut self) {
        self.lines = parse_pasted_lyrics(&self.lyrics_raw);
        self.next_untimed = 0;
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
                self.next_untimed = self
                    .lines
                    .iter()
                    .position(|l| l.start.is_none())
                    .unwrap_or(self.lines.len());
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
        self.lines[self.next_untimed].start = Some(pos);
        self.next_untimed += 1;

        if self.next_untimed >= self.lines.len() && !self.lines.is_empty() {
            // Just finished the last line - open the fine-tune-words panel
            // automatically. It'll track along with playback from here, so
            // the user can keep playing and just click words as they come
            // up rather than having to manually select each line.
            self.word_tap_line = Some(0);
            self.status = "All lines timed! Keep playing - click words as they're sung to \
                           fine-tune them; it'll follow the song automatically."
                .to_string();
        }
    }

    /// While the fine-tune-words panel is open and the song is playing,
    /// keep it pointed at whichever line is currently active, so the user
    /// can just play through and click words without manually reselecting
    /// a line every time the song moves on to the next one.
    fn auto_follow_word_tap_line(&mut self) {
        if self.word_tap_line.is_none() {
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
            timed.push(TimedLine::with_overrides(
                line.text.clone(),
                start,
                end,
                line.singer,
                line.word_overrides.clone(),
                line.sing_end_override,
                line.starts_new_block,
            ));
            indices.push(orig_idx);
        }
        (timed, indices)
    }

    fn retap_line(&mut self, idx: usize) {
        let Some(audio) = &self.audio else { return };
        if !audio.is_playing() {
            self.status = "Press Play first, then tap.".to_string();
            return;
        }
        let pos = audio.position();
        self.lines[idx].start = Some(pos);
        self.next_untimed = self
            .lines
            .iter()
            .position(|l| l.start.is_none())
            .unwrap_or(self.lines.len());
    }

    fn reset_timing(&mut self) {
        for l in &mut self.lines {
            l.start = None;
        }
        self.next_untimed = 0;
        self.status = "Timing cleared.".to_string();
    }

    fn tap_word(&mut self, line_idx: usize, word_idx: usize) {
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

    fn reset_word_overrides(&mut self, line_idx: usize) {
        if let Some(line) = self.lines.get_mut(line_idx) {
            for o in line.word_overrides.iter_mut() {
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
        if let Some(line) = self.lines.get_mut(line_idx) {
            line.sing_end_override = Some(pos);
        }
    }

    fn start_video_export(&mut self) {
        if self.video_export.is_some() {
            self.status = "A video export is already in progress.".to_string();
            return;
        }
        let timed_count = self.lines.iter().filter(|l| l.start.is_some()).count();
        if timed_count == 0 {
            self.status = "No lines are timed yet - tap along with the song first.".to_string();
            return;
        }
        let Some(audio_path) = self
            .audio
            .as_ref()
            .and_then(|a| a.path())
            .map(|p| p.to_path_buf())
        else {
            self.status = "Load an audio file first - the video needs it for sound.".to_string();
            return;
        };

        let (timed, total_duration) = self.resolved();
        let palette = self.video_palette();
        let title = if self.title.trim().is_empty() {
            None
        } else {
            Some(self.title.trim().to_string())
        };
        let artist = if self.artist.trim().is_empty() {
            None
        } else {
            Some(self.artist.trim().to_string())
        };
        let resolution = self.video_resolution;

        let default_name = self
            .audio
            .as_ref()
            .and_then(|a| a.file_name())
            .map(|n| {
                let stem = n.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or(n);
                format!("{stem}.mp4")
            })
            .unwrap_or_else(|| "karaoke.mp4".to_string());

        let Some(output_path) = rfd::FileDialog::new()
            .set_file_name(&default_name)
            .add_filter("MP4 video", &["mp4"])
            .save_file()
        else {
            return;
        };

        let progress = Arc::new(AtomicU32::new(0));
        let result = Arc::new(Mutex::new(None));
        let progress_clone = progress.clone();
        let result_clone = result.clone();

        self.status = "Rendering video… this can take a while for longer songs.".to_string();
        self.video_export = Some(VideoExportHandle { progress, result });

        std::thread::spawn(move || {
            let r = video::render_video(
                &timed,
                total_duration,
                &palette,
                title.as_deref(),
                artist.as_deref(),
                resolution,
                30,
                &audio_path,
                &output_path,
                |p| {
                    progress_clone.store((p * 1000.0) as u32, Ordering::Relaxed);
                },
            );
            let mapped = r.map(|()| output_path).map_err(|e| e.to_string());
            *result_clone.lock().unwrap() = Some(mapped);
        });
    }

    /// Checks on a running video export, if any, updating status when it
    /// finishes. Returns true while an export is still in progress (so the
    /// caller knows to keep repainting for the progress bar).
    fn poll_video_export(&mut self) -> bool {
        let Some(handle) = &self.video_export else {
            return false;
        };
        let finished = handle.result.lock().unwrap().take();
        if let Some(result) = finished {
            match result {
                Ok(path) => {
                    self.status = format!(
                        "Saved {} - a complete standalone video, ready to share or upload.",
                        path.display()
                    );
                }
                Err(e) => {
                    self.status = format!("Video export failed: {e}");
                }
            }
            self.video_export = None;
            false
        } else {
            true
        }
    }

    fn export(&mut self) {
        let timed_count = self.lines.iter().filter(|l| l.start.is_some()).count();
        if timed_count == 0 {
            self.status = "No lines are timed yet - tap along with the song first.".to_string();
            return;
        }

        let (timed, total_duration) = self.resolved();

        let default_name = self
            .audio
            .as_ref()
            .and_then(|a| a.file_name())
            .map(|n| {
                let stem = n.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or(n);
                format!("{stem}.cdg")
            })
            .unwrap_or_else(|| "karaoke.cdg".to_string());

        let Some(path) = rfd::FileDialog::new()
            .set_file_name(&default_name)
            .add_filter("CD+Graphics", &["cdg"])
            .save_file()
        else {
            return;
        };

        let title = if self.title.trim().is_empty() {
            None
        } else {
            Some(self.title.trim())
        };
        let artist = if self.artist.trim().is_empty() {
            None
        } else {
            Some(self.artist.trim())
        };

        let bytes = export::render_cdg(&timed, total_duration, &self.palette(), title, artist);
        match std::fs::write(&path, &bytes) {
            Ok(()) => {
                self.status = format!(
                    "Saved {} ({:.1}s of graphics). To play it, put an audio file with the \
                     SAME name next to it (e.g. {}.mp3 alongside {}) - most karaoke players \
                     look for that pair automatically.",
                    path.display(),
                    total_duration,
                    path.file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    path.file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default(),
                );
            }
            Err(e) => {
                self.status = format!("Couldn't write file: {e}");
            }
        }
    }

    /// Draws the live "what will this look like" preview: a mock TV screen
    /// showing exactly what the exported CDG will show at the current
    /// playback position (or position 0 if nothing is playing).
    fn draw_preview(&self, ui: &mut egui::Ui) {
        let t = self.audio.as_ref().map(|a| a.position()).unwrap_or(0.0);
        let (timed, _total) = self.resolved();
        let card_end = export::title_card_end(&timed);

        let width = ui.available_width().min(480.0);
        let height = width * 9.0 / 16.0;

        egui::Frame::default().fill(self.color_bg).show(ui, |ui| {
            ui.set_min_size(egui::vec2(width, height));
            ui.set_max_size(egui::vec2(width, height));

            let has_title_card = !self.title.trim().is_empty() || !self.artist.trim().is_empty();

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

                        for (slot, &idx) in block.iter().enumerate() {
                            let line = &timed[idx];
                            let (unsung, highlight) = self.singer_colors(line.singer);
                            let normalized = lyrics::normalize_text(&line.text);
                            match slot.cmp(&slot_in_block) {
                                std::cmp::Ordering::Less => {
                                    // Already sung - shown fully in the highlight color.
                                    ui.colored_label(
                                        highlight,
                                        egui::RichText::new(normalized).size(18.0),
                                    );
                                }
                                std::cmp::Ordering::Equal => {
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
        });
    }
}

impl eframe::App for KaraokeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
        if self.poll_video_export() {
            ctx.request_repaint();
        }
        self.auto_follow_word_tap_line();

        // Keyboard shortcuts - only when no text field etc. has focus, so
        // typing a space in the lyrics box or title/artist fields doesn't
        // accidentally trigger a tap.
        let typing = ctx.memory(|m| m.focused().is_some());
        if !typing {
            if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
                self.tap_next();
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
        });

        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| {
            ui.add_space(4.0);
            let exporting_video = self.video_export.is_some();
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!exporting_video, |ui| {
                    if ui.button("Export .cdg…").clicked() {
                        self.export();
                    }
                    if ui.button("Reset all timing").clicked() {
                        self.reset_timing();
                    }
                    ui.separator();
                    egui::ComboBox::from_id_source("video_res")
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
                    if ui.button("Export video (.mp4)…").clicked() {
                        self.start_video_export();
                    }
                });
            });
            if let Some(handle) = &self.video_export {
                let frac = handle.progress.load(Ordering::Relaxed) as f32 / 1000.0;
                ui.add(egui::ProgressBar::new(frac).show_percentage());
            }
            if !self.status.is_empty() {
                ui.label(&self.status);
            }
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
                    "Press Play, then press Space (or click below) the instant each new \
                     line starts being sung. Lines fill in top to bottom automatically - \
                     missed one? Drag the seek bar back a few seconds and try again.",
                );
                let can_tap = self.audio.as_ref().map(|a| a.is_playing()).unwrap_or(false)
                    && self.next_untimed < self.lines.len();
                ui.add_enabled_ui(can_tap, |ui| {
                    let label = if self.next_untimed < self.lines.len() {
                        format!(
                            "⏱ Tap next line ({}/{})  [Space]",
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
                            if ui.small_button("Close").clicked() {
                                self.word_tap_line = None;
                            }
                        });
                        ui.label(
                            "Play the song and just click each word the instant it's sung - \
                             this panel follows along automatically as the song moves from \
                             line to line, so there's no need to reselect a line yourself. \
                             Click a word again to retime it. Green = manually timed; the \
                             rest still use the automatic estimate.",
                        );
                        let words: Vec<String> = self.lines[i]
                            .text
                            .split_whitespace()
                            .map(|s| s.to_string())
                            .collect();
                        ui.horizontal_wrapped(|ui| {
                            for (w_idx, word) in words.iter().enumerate() {
                                let manual =
                                    self.lines[i].word_overrides.get(w_idx).copied().flatten();
                                let text = match manual {
                                    Some(t) => format!("{word} ({t:.1}s)"),
                                    None => word.clone(),
                                };
                                let button = egui::Button::new(text).fill(if manual.is_some() {
                                    egui::Color32::from_rgb(35, 90, 45)
                                } else {
                                    ui.visuals().widgets.inactive.weak_bg_fill
                                });
                                if ui.add(button).clicked() {
                                    self.tap_word(i, w_idx);
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
                        if ui.small_button("Reset word & end timing for this line").clicked() {
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
                    egui::Grid::new("lines_grid")
                        .num_columns(6)
                        .striped(true)
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            let mut retap_idx: Option<usize> = None;
                            let mut words_idx: Option<usize> = None;
                            let mut nudge: Option<(usize, f64)> = None;
                            let mut clear_idx: Option<usize> = None;

                            for i in 0..self.lines.len() {
                                let is_next = i == self.next_untimed;
                                let label = match self.lines[i].start {
                                    Some(s) => format_time(s),
                                    None => "—".to_string(),
                                };
                                ui.label(label);

                                ui.scope(|ui| {
                                    ui.set_max_width(260.0);
                                    let text_label = if is_next {
                                        egui::RichText::new(&self.lines[i].text).strong()
                                    } else {
                                        egui::RichText::new(&self.lines[i].text)
                                    };
                                    ui.add(egui::Label::new(text_label).wrap());
                                });

                                egui::ComboBox::from_id_source(("singer", i))
                                    .width(82.0)
                                    .selected_text(match self.lines[i].singer {
                                        Singer::Male => "Male",
                                        Singer::Female => "Female",
                                        Singer::Duet => "Duet",
                                        Singer::Screaming => "Screaming",
                                    })
                                    .show_ui(ui, |ui| {
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
                            }
                            if let Some(i) = clear_idx {
                                self.lines[i].start = None;
                                self.next_untimed = self
                                    .lines
                                    .iter()
                                    .position(|l| l.start.is_none())
                                    .unwrap_or(self.lines.len());
                            }
                        });
                });
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1350.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Abyssal CDG Creator",
        options,
        Box::new(|_cc| Ok(Box::new(KaraokeApp::new()))),
    )
}
