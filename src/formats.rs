//! Import (and some export) support for common lyric/timing file formats,
//! so real-world lyric files can be loaded directly instead of always
//! starting from a blank paste box.
//!
//! Supported:
//! - **LRC** (`.lrc`) - both basic line-level LRC1 and word-level
//!   "enhanced" LRC2 (`<mm:ss.xx>` tags mid-line), which reads straight
//!   into [`LyricLine::word_overrides`].
//! - **UltraStar** (`.txt`) - beat-based note format. Pitch data is parsed
//!   (so the file structure is understood) and then discarded, since this
//!   app has no pitch-display feature. `P1`/`P2`/`P3` duet markers are
//!   mapped onto our own [`Singer`] enum (Male/Female/Duet).
//! - **KOK** (`.kok`, DEL MP3 Karaoke / KaraWin) - word-level timestamps
//!   only; see the note on [`import_kok`] before relying on its line
//!   grouping for a file with real verse structure.
//!
//! **Not implemented:** KaraokeBuilder Studio's `.kbp` is a complex,
//! proprietary, page/syllable-based project format we don't have confident
//! first-hand grammar knowledge of - rather than guess at it and risk
//! silently wrong timings, it's intentionally left out pending a real
//! sample file or spec to verify against. "PowerKaraoke" turned out not to
//! be a single fixed file format at all (it's a configurable import wizard
//! in that software, where the user picks time format/separators) - there's
//! no fixed grammar to target, so it isn't implemented as its own format
//! here either.

use crate::lyrics::{LyricLine, Singer, TimedLine};

/// A line-start gap bigger than this (in seconds) is treated as a likely
/// verse/section break for formats that don't have an explicit blank-line
/// convention of their own (unlike our native plain-text format, which
/// gets this from actual blank lines - see
/// [`crate::lyrics::parse_pasted_lyrics`]).
const IMPORTED_BLOCK_GAP_THRESHOLD: f64 = 6.0;

/// How far before the next line's start an imported line's estimated end
/// is placed - see [`estimate_imported_sing_duration`].
const IMPORTED_END_LEAD_SECS: f64 = 0.2;

/// Generous per-word ceiling on an imported line's estimated singing time,
/// for formats that only give a line's start - stops a line right before a
/// long instrumental break from being estimated to last the whole break.
/// Deliberately much looser than [`crate::lyrics::SECONDS_PER_WORD`]: the
/// estimate is meant to cover slow/held lines too, since auto-align then
/// narrows it down within that window.
const IMPORTED_MAX_SECS_PER_WORD: f64 = 1.0;

/// Floor for that same ceiling, so a one- or two-word line isn't capped
/// shorter than a typical held phrase.
const IMPORTED_MIN_SING_SECS: f64 = 2.0;

/// Best-effort guess at how long an imported line is sung, in seconds from
/// `start`, for formats with no real end time of their own - stored as
/// [`LyricLine::estimated_sing_duration`], so it's an "auto" estimate (not
/// a manual end) that's still much closer than a bare word count.
///
/// - With per-word times (`last_word` = the last word's start and text),
///   the line ends shortly after its last word starts.
/// - With only a line start, it runs up to just before the next line's
///   start (`next_start`), capped by a generous words-per-second limit.
///   Without a next line there's nothing better to go on than the plain
///   word-count estimate, so this returns `None`.
///
/// Either way it never runs past [`IMPORTED_END_LEAD_SECS`] before the next
/// line starts.
fn estimate_imported_sing_duration(
    start: f64,
    word_count: usize,
    last_word: Option<(f64, &str)>,
    next_start: Option<f64>,
) -> Option<f64> {
    let ceiling = next_start.map(|n| n - start - IMPORTED_END_LEAD_SECS);
    let dur = match last_word {
        Some((word_start, word)) => {
            let word_len = (word.chars().count() as f64 * 0.1).clamp(0.4, 2.0);
            let dur = word_start - start + word_len;
            // Never cut off before the last word has even started.
            let floor = word_start - start + 0.05;
            ceiling.map_or(dur, |c| dur.min(c)).max(floor)
        }
        None => {
            let cap = (word_count as f64 * IMPORTED_MAX_SECS_PER_WORD).max(IMPORTED_MIN_SING_SECS);
            ceiling?.min(cap)
        }
    };
    (dur.is_finite() && dur > 0.05).then_some(dur)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LyricFormat {
    PlainText,
    Lrc,
    UltraStar,
    Kok,
}

impl LyricFormat {
    pub fn label(self) -> &'static str {
        match self {
            LyricFormat::PlainText => "Plain text",
            LyricFormat::Lrc => "LRC",
            LyricFormat::UltraStar => "UltraStar",
            LyricFormat::Kok => "KOK",
        }
    }
}

/// Sniff which format `text` is likely in, so "Load lyrics file" doesn't
/// need the user to pick manually. Falls back to [`LyricFormat::PlainText`]
/// if nothing distinctive is found - plain text is a valid format in its
/// own right, not just a failure case.
pub fn detect_format(text: &str) -> LyricFormat {
    let head: String = text.chars().take(4000).collect();

    if head.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with("#TITLE:")
            || l.starts_with("#BPM:")
            || l.starts_with("#GAP:")
            || l.starts_with("#ARTIST:")
    }) {
        return LyricFormat::UltraStar;
    }

    if head.lines().any(is_lrc_timestamp_line) {
        return LyricFormat::Lrc;
    }

    if looks_like_kok(&head) {
        return LyricFormat::Kok;
    }

    LyricFormat::PlainText
}

fn is_lrc_timestamp_line(line: &str) -> bool {
    let line = line.trim_start();
    let Some(rest) = line.strip_prefix('[') else {
        return false;
    };
    let Some(end) = rest.find(']') else {
        return false;
    };
    parse_lrc_time_tag(&rest[..end]).is_some()
}

/// Looks for a `digits,digits;` token - KOK's distinctive
/// comma-decimal-seconds timestamp shape, unlikely to appear by chance in
/// plain lyrics or the other supported formats.
fn looks_like_kok(head: &str) -> bool {
    let bytes = head.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b',' {
                let frac_start = i + 1;
                let mut j = frac_start;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > frac_start && j < bytes.len() && bytes[j] == b';' {
                    return true;
                }
            }
        } else {
            i += 1;
        }
    }
    false
}

fn is_new_block(prev_start: f64, cur_start: f64) -> bool {
    (cur_start - prev_start) > IMPORTED_BLOCK_GAP_THRESHOLD
}

fn split_mmss(secs: f64) -> (u64, f64) {
    let secs = secs.max(0.0);
    let mm = (secs / 60.0).floor() as u64;
    let ss = secs - mm as f64 * 60.0;
    (mm, ss)
}

// ---------------------------------------------------------------------
// LRC (LRC1 + LRC2)
// ---------------------------------------------------------------------

/// Parses one `[mm:ss.xx]`-style tag (also accepts `mm:ss` with no
/// fractional part, and the rarer `mm:ss:xx` colon-separated centiseconds
/// some LRC writers use). Returns `None` for non-timestamp tags like
/// `[ar:Some Artist]`, so metadata lines can be told apart from lyric lines.
fn parse_lrc_time_tag(tag: &str) -> Option<f64> {
    let tag = tag.trim();
    let (mm, rest) = tag.split_once(':')?;
    if mm.is_empty() || !mm.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mm: f64 = mm.parse().ok()?;
    let rest_norm = rest.replacen(':', ".", 1);
    let ss: f64 = rest_norm.parse().ok()?;
    // `f64::parse` accepts "nan"/"inf"/"infinity" (case-insensitive) as
    // valid floats - `mm`'s own all-ASCII-digit check above already rules
    // those out for the minutes part, but `ss` (the seconds/centiseconds
    // part after the colon) has no equivalent guard, so a tag like
    // `[00:nan]` would otherwise silently produce a NaN timestamp that
    // poisons the `sort_by` a few lines down in `import_lrc` (`NaN.partial_cmp`
    // is `None`, and that sort's `.unwrap()` panics on it) - same class of
    // bug `lyrics::parse_timecode` already guards against for manual entry.
    if !mm.is_finite() || !ss.is_finite() || mm < 0.0 || ss < 0.0 {
        return None;
    }
    Some(mm * 60.0 + ss)
}

/// Extracts LRC2 inline word tags (`<mm:ss.xx>`) from an already
/// line-timestamp-stripped line, returning the plain text (tags removed)
/// and one timestamp per tag, in order. Returns an empty timestamp list if
/// there are no inline tags at all (plain LRC1 line).
fn parse_lrc2_inline_tags(content: &str) -> (String, Vec<f64>) {
    if !content.contains('<') {
        return (content.to_string(), Vec::new());
    }
    let mut plain = String::new();
    let mut word_times = Vec::new();
    let mut rest = content;
    while let Some(lt) = rest.find('<') {
        plain.push_str(&rest[..lt]);
        rest = &rest[lt + 1..];
        let Some(gt) = rest.find('>') else {
            // Unclosed tag - stop trying to parse further tags, keep the
            // rest of the line as plain text rather than losing it.
            plain.push('<');
            plain.push_str(rest);
            return (plain, word_times);
        };
        let tag = &rest[..gt];
        rest = &rest[gt + 1..];
        if let Some(t) = parse_lrc_time_tag(tag) {
            // A tag between two letters (`<..>beau<..>ti<..>ful`) is a
            // syllable break inside one word - mark it so that word gets one
            // timing unit per syllable instead of the extra tags making the
            // whole line's word timing unusable.
            let mid_word = plain.chars().last().is_some_and(|c| !c.is_whitespace())
                && rest
                    .chars()
                    .next()
                    .is_some_and(|c| !c.is_whitespace() && c != '<');
            if mid_word {
                plain.push(crate::lyrics::SYLLABLE_MARK);
            }
            word_times.push(t);
        }
    }
    plain.push_str(rest);
    (plain, word_times)
}

/// Imports an LRC file (LRC1 line-level, or LRC2 with inline word tags -
/// both are handled by the same function, since LRC2 is a strict superset
/// of LRC1's line format). Metadata tags (`[ar:]`, `[ti:]`, `[offset:]`
/// etc) are recognized and skipped rather than mis-parsed as zero
/// timestamps. A line with multiple leading time tags (used for a chorus
/// that repeats at several points) becomes one entry per timestamp.
pub fn import_lrc(text: &str) -> Vec<LyricLine> {
    let mut entries: Vec<(f64, String)> = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let mut rest = line;
        let mut times: Vec<f64> = Vec::new();
        while let Some(r) = rest.strip_prefix('[') {
            let Some(end) = r.find(']') else { break };
            let tag = &r[..end];
            match parse_lrc_time_tag(tag) {
                Some(secs) => {
                    times.push(secs);
                    rest = &r[end + 1..];
                }
                None => break, // metadata tag like [ar:...] - not a lyric line
            }
        }
        if times.is_empty() {
            continue;
        }
        for t in times {
            entries.push((t, rest.to_string()));
        }
    }

    entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut result = Vec::with_capacity(entries.len());
    for (i, (start, content)) in entries.iter().enumerate() {
        let (text_only, mut word_times) = parse_lrc2_inline_tags(content);
        let trimmed = text_only.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut line = LyricLine::new(trimmed);
        line.start = Some(*start);
        line.starts_new_block = i == 0 || is_new_block(entries[i - 1].0, *start);
        let word_count = line.word_overrides.len();
        // Enhanced LRC commonly puts one extra tag after the last word,
        // marking when the line ends - a real end time, not a guess.
        if word_count > 0 && word_times.len() == word_count + 1 {
            let end = word_times.pop().unwrap();
            if end > word_times[word_count - 1] {
                line.sing_end_override = Some(end);
            }
        }
        let has_word_times = word_times.len() == word_count;
        if has_word_times {
            line.word_overrides = word_times.into_iter().map(Some).collect();
        }
        if line.sing_end_override.is_none() {
            // Blank timed lines (instrumental markers) still count as "the
            // next line" here - they're exactly where singing stops.
            let next_start = entries.get(i + 1).map(|e| e.0);
            let last_word = if has_word_times {
                line.word_overrides
                    .last()
                    .copied()
                    .flatten()
                    .zip(line.words().last().copied())
            } else {
                None
            };
            line.estimated_sing_duration =
                estimate_imported_sing_duration(*start, word_count, last_word, next_start);
        }
        result.push(line);
    }
    result
}

/// Exports timed lines back to LRC text, the inverse of [`import_lrc`] -
/// with `[ti:]`/`[ar:]` metadata tags when `title`/`artist` are given.
/// `enhanced = true` writes LRC2 inline word tags (from
/// [`crate::lyrics::word_timings`]); `false` writes plain line-level LRC1.
pub fn export_lrc(
    timed: &[TimedLine],
    enhanced: bool,
    title: Option<&str>,
    artist: Option<&str>,
) -> String {
    let mut out = String::new();
    if let Some(t) = title {
        out.push_str(&format!("[ti:{t}]\n"));
    }
    if let Some(a) = artist {
        out.push_str(&format!("[ar:{a}]\n"));
    }
    for line in timed {
        let (mm, ss) = split_mmss(line.start);
        out.push_str(&format!("[{mm:02}:{ss:05.2}]"));
        if enhanced {
            for w in crate::lyrics::word_timings(line) {
                let (wm, ws) = split_mmss(w.highlight_at);
                // A syllable (`beau|ti|ful`) gets its own tag mid-word, with
                // no space until its word ends - LRC2's own syllable form.
                let sep = if w.ends_word { " " } else { "" };
                out.push_str(&format!("<{wm:02}:{ws:05.2}>{}{sep}", w.text));
            }
            out.push('\n');
        } else {
            out.push_str(&crate::lyrics::normalize_text(&line.text));
            out.push('\n');
        }
    }
    out
}

// ---------------------------------------------------------------------
// Subtitles: SRT / ASS
// ---------------------------------------------------------------------

/// How long before a line starts its `.ass` dialogue appears (unsung), so
/// the singer can read ahead - the same idea as the video's dim upcoming
/// lines. Shortened for a line right after another so they don't stack up.
const ASS_LEAD_IN_SECS: f64 = 1.0;
/// How long a sung line stays up after it finishes, in both formats.
const SUBTITLE_TAIL_SECS: f64 = 0.5;
/// A subtitle cue is never shorter than this, so a very short (mis-tapped)
/// line still flashes up readably.
const MIN_CUE_SECS: f64 = 0.5;

/// `HH:MM:SS,mmm` (SRT).
fn srt_time(secs: f64) -> String {
    let ms = (secs.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// `H:MM:SS.cc` (ASS - centiseconds).
fn ass_time(secs: f64) -> String {
    let cs = (secs.max(0.0) * 100.0).round() as u64;
    format!(
        "{}:{:02}:{:02}.{:02}",
        cs / 360_000,
        cs / 6000 % 60,
        cs / 100 % 60,
        cs % 100
    )
}

/// The title/artist card text, if there's anything to show.
fn title_card_text(title: Option<&str>, artist: Option<&str>) -> Option<(String, Option<String>)> {
    match (title, artist) {
        (None, None) => None,
        (t, a) => Some((
            t.unwrap_or_default().to_string(),
            a.map(|a| format!("by {a}")),
        )),
    }
}

/// Exports SubRip (`.srt`) subtitles: one cue per lyric line (`timed`
/// should be the main and backing tracks merged and sorted by start, the
/// same input as [`export_lrc`]), shown from the line's start until just
/// after it's sung, plus a title/artist cue up to `card_end`. Plain text -
/// SRT has no word timing, so for a karaoke-style word wipe use
/// [`export_ass`] instead.
pub fn export_srt(
    timed: &[TimedLine],
    title: Option<&str>,
    artist: Option<&str>,
    card_end: f64,
) -> String {
    let mut cues: Vec<(f64, f64, String)> = Vec::new();
    if let Some((t, a)) = title_card_text(title, artist) {
        let text = [Some(t), a]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let first_start = timed.first().map(|l| l.start).unwrap_or(card_end);
        let end = card_end.min(first_start);
        if end > 0.0 && !text.is_empty() {
            cues.push((0.0, end, text));
        }
    }
    for line in timed {
        let end = (line.sing_end + SUBTITLE_TAIL_SECS).max(line.start + MIN_CUE_SECS);
        cues.push((line.start, end, crate::lyrics::normalize_text(&line.text)));
    }
    let mut out = String::new();
    for (n, (start, end, text)) in cues.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            n + 1,
            srt_time(*start),
            srt_time(*end),
            text
        ));
    }
    out
}

/// `&HAABBGGRR` (ASS color: alpha then BGR, alpha 00 = opaque).
fn ass_color(c: crate::video::Rgb8) -> String {
    format!("&H00{:02X}{:02X}{:02X}", c.b, c.g, c.r)
}

/// Makes lyric text safe inside an ASS dialogue line: `{`/`}` would start
/// an override block and `\` an escape, so they're swapped for look-alikes.
fn ass_escape(text: &str) -> String {
    text.replace('\\', "/").replace('{', "(").replace('}', ")")
}

/// The ASS style name used for a voice - one per base [`Singer`] (custom
/// singer names share their base voice's color, same as everywhere else).
fn ass_style_name(singer: Singer, backing: bool) -> String {
    if backing && singer == Singer::Backing {
        "Backing lane".to_string()
    } else if backing {
        format!("{} backing lane", singer.label())
    } else {
        singer.label().to_string()
    }
}

/// One `Dialogue:` line with `\kf` karaoke tags: shown from `show_from`
/// (unsung), then each word fills left to right over its own highlight
/// window, with a plain `\k` pause wherever a word's highlight ends before
/// the next one starts (a manually set word end), held until `show_until`.
fn ass_dialogue(line: &TimedLine, style: &str, show_from: f64, show_until: f64) -> String {
    let cs = |t: f64| ((t - show_from) * 100.0).round().max(0.0) as i64;
    let mut text = String::new();
    let words = crate::lyrics::word_timings(line);
    let mut cursor = 0i64;
    for (i, w) in words.iter().enumerate() {
        let start = cs(w.highlight_at).max(cursor);
        if start > cursor {
            text.push_str(&format!("{{\\k{}}}", start - cursor));
        }
        let held = cs(w.held_until).max(start);
        let sep = if w.ends_word && i + 1 < words.len() {
            " "
        } else {
            ""
        };
        text.push_str(&format!(
            "{{\\kf{}}}{}{sep}",
            held - start,
            ass_escape(w.text)
        ));
        cursor = held;
    }
    format!(
        "Dialogue: 0,{},{},{style},,0,0,0,karaoke,{text}\n",
        ass_time(show_from),
        ass_time(show_until)
    )
}

/// Exports Advanced SubStation Alpha (`.ass`) subtitles with karaoke
/// timing - players and editors that support ASS (VLC, mpv, Aegisub,
/// most video editors, YouTube via conversion) sweep each word from its
/// unsung to its sung color as it's sung, using `\kf` tags from the same
/// word timing the `.cdg`/video wipe uses. One style per voice in use, in
/// the video palette's colors; backing lines get their own smaller styles
/// so they read as secondary. `font_name` is the lyric font family (a
/// player without it falls back to its default).
pub fn export_ass(
    main: &[TimedLine],
    backing: &[TimedLine],
    palette: &crate::video::VideoPalette,
    font_name: &str,
    title: Option<&str>,
    artist: Option<&str>,
    card_end: f64,
) -> String {
    let font = font_name.replace(',', " ");
    let mut out = String::new();
    out.push_str("[Script Info]\n");
    out.push_str("; Generated by Abyssal CDG Creator\n");
    if let Some(t) = title {
        out.push_str(&format!("Title: {}\n", t.replace(['\n', '\r'], " ")));
    }
    out.push_str(
        "ScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\nWrapStyle: 0\n\
         ScaledBorderAndShadow: yes\n\n",
    );

    out.push_str("[V4+ Styles]\n");
    out.push_str(
        "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, \
         BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, \
         BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n",
    );
    // PrimaryColour = sung (what \k fills to), SecondaryColour = unsung.
    let style = |name: &str,
                 size: u32,
                 sung: crate::video::Rgb8,
                 unsung: crate::video::Rgb8,
                 align: u8,
                 margin_v: u32| {
        format!(
            "Style: {name},{font},{size},{},{},&H00000000,&H80000000,-1,0,0,0,100,100,0,0,1,4,2,{align},60,60,{margin_v},1\n",
            ass_color(sung),
            ass_color(unsung)
        )
    };
    let mut styles_used: Vec<(Singer, bool)> = main
        .iter()
        .map(|l| (l.singer, false))
        .chain(backing.iter().map(|l| (l.singer, true)))
        .collect();
    styles_used.sort_by_key(|(s, b)| (*b, Singer::ALL.iter().position(|x| x == s)));
    styles_used.dedup();
    for &(singer, is_backing) in &styles_used {
        let (unsung, sung) = palette.singer_colors(singer);
        let name = ass_style_name(singer, is_backing);
        out.push_str(&if is_backing {
            style(&name, 54, sung, unsung, 2, 40)
        } else {
            style(&name, 72, sung, unsung, 2, 140)
        });
    }
    out.push_str(&style("Title", 96, palette.title, palette.title, 5, 0));
    out.push_str(&style("Artist", 60, palette.artist, palette.artist, 5, 0));

    out.push_str("\n[Events]\n");
    out.push_str(
        "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
    );
    if let Some((t, a)) = title_card_text(title, artist) {
        let first_start = main.first().map(|l| l.start).unwrap_or(card_end);
        let end = card_end.min(first_start);
        if end > 0.0 {
            if !t.is_empty() {
                out.push_str(&format!(
                    "Dialogue: 0,{},{},Title,,0,0,0,,{{\\pos(960,480)}}{}\n",
                    ass_time(0.0),
                    ass_time(end),
                    ass_escape(&t)
                ));
            }
            if let Some(a) = a {
                out.push_str(&format!(
                    "Dialogue: 0,{},{},Artist,,0,0,0,,{{\\pos(960,600)}}{}\n",
                    ass_time(0.0),
                    ass_time(end),
                    ass_escape(&a)
                ));
            }
        }
    }
    for (track, is_backing) in [(main, false), (backing, true)] {
        let mut prev_end = card_end.min(track.first().map(|l| l.start).unwrap_or(0.0));
        for line in track {
            // Lead in, but not back past where the previous line on this
            // track finished (no stacking two lines of the same track).
            let floor = if is_backing { 0.0 } else { prev_end };
            let show_from = (line.start - ASS_LEAD_IN_SECS)
                .max(floor)
                .max(0.0)
                .min(line.start);
            let show_until = (line.sing_end + SUBTITLE_TAIL_SECS).max(line.start + MIN_CUE_SECS);
            out.push_str(&ass_dialogue(
                line,
                &ass_style_name(line.singer, is_backing),
                show_from,
                show_until,
            ));
            prev_end = line.sing_end;
        }
    }
    out
}

/// The outro card's text, top to bottom: its own lines, then (if it
/// includes them) the title and "by artist".
fn outro_text_lines(
    outro: &crate::export::Outro,
    title: Option<&str>,
    artist: Option<&str>,
) -> Vec<(String, bool)> {
    // (text, is_title_or_artist)
    let mut out: Vec<(String, bool)> = outro.lines.iter().map(|l| (l.clone(), false)).collect();
    if outro.show_title {
        if let Some(t) = title {
            out.push((t.to_string(), true));
        }
        if let Some(a) = artist {
            out.push((format!("by {a}"), true));
        }
    }
    out
}

/// Appends the outro card to an [`export_srt`] result as one more cue
/// (numbered after the existing ones), over the card's own window.
pub fn append_srt_outro(
    srt: &mut String,
    outro: &crate::export::Outro,
    title: Option<&str>,
    artist: Option<&str>,
) {
    let text: Vec<String> = outro_text_lines(outro, title, artist)
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    if text.is_empty() {
        return;
    }
    let n = srt.matches(" --> ").count() + 1;
    srt.push_str(&format!(
        "{n}\n{} --> {}\n{}\n\n",
        srt_time(outro.start),
        srt_time(outro.end),
        text.join("\n")
    ));
}

/// Appends the outro card to an [`export_ass`] result (whose `[Events]`
/// section is last) as centered dialogue lines in the Title/Artist
/// styles, stacked like the title card, fading in.
pub fn append_ass_outro(
    ass: &mut String,
    outro: &crate::export::Outro,
    title: Option<&str>,
    artist: Option<&str>,
) {
    let lines = outro_text_lines(outro, title, artist);
    let step = 110.0;
    let top = 540.0 - step * (lines.len().saturating_sub(1)) as f64 / 2.0;
    for (k, (text, is_meta)) in lines.iter().enumerate() {
        let style = if *is_meta && text.starts_with("by ") {
            "Artist"
        } else {
            "Title"
        };
        ass.push_str(&format!(
            "Dialogue: 0,{},{},{style},,0,0,0,,{{\\pos(960,{:.0})\\fad(600,0)}}{}\n",
            ass_time(outro.start),
            ass_time(outro.end),
            top + step * k as f64,
            ass_escape(text)
        ));
    }
}

// ---------------------------------------------------------------------
// UltraStar
// ---------------------------------------------------------------------

/// UltraStar's beat-to-seconds conversion: the BPM stored in the file is
/// (by long-standing convention across UltraStar/USDX/Vocaluxe/Performous)
/// 4x the "musical" BPM, giving finer note-timing resolution than whole
/// beats would allow. `gap_ms` is the offset before beat 0.
fn ultrastar_beat_to_secs(beat: f64, bpm: f64, gap_ms: f64) -> f64 {
    gap_ms / 1000.0 + beat * 60.0 / (bpm * 4.0)
}

/// Imports an UltraStar `.txt` file. Only a single constant BPM is
/// supported (mid-song BPM-change lines, `B BEAT NEWBPM`, are rare in
/// practice and not handled here); pitch is parsed as part of each note's
/// fields but discarded, since this app doesn't have a pitch-display
/// feature. Returns `Err` with a human-readable reason for genuinely
/// malformed input (missing `#BPM:`) rather than silently producing
/// nonsense timing.
pub fn import_ultrastar(text: &str) -> Result<Vec<LyricLine>, String> {
    let mut bpm: Option<f64> = None;
    let mut gap_ms: f64 = 0.0;

    for raw in text.lines() {
        let l = raw.trim();
        if let Some(v) = l.strip_prefix("#BPM:") {
            // Some files use a comma as the decimal separator here too.
            bpm = v.trim().replace(',', ".").parse().ok();
        } else if let Some(v) = l.strip_prefix("#GAP:") {
            // `f64::parse` accepts "nan"/"inf"/"infinity" (case-insensitive)
            // as valid floats, so `.unwrap_or(0.0)` alone would not catch a
            // `#GAP:nan` header - it only covers genuinely unparseable text.
            let g: f64 = v.trim().replace(',', ".").parse().unwrap_or(0.0);
            gap_ms = if g.is_finite() { g } else { 0.0 };
        }
    }
    let bpm =
        bpm.ok_or_else(|| "missing #BPM: header - not a recognizable UltraStar file".to_string())?;
    // `!bpm.is_finite()` catches "nan"/"inf"/"-inf" values, which `f64::parse`
    // accepts as valid floats but which `bpm <= 0.0` alone would not reject
    // (every comparison with NaN is `false`) - left unchecked, a NaN BPM
    // propagates into every line's timing via `ultrastar_beat_to_secs` and
    // poisons the NaN-sensitive `sort_by` this file uses elsewhere.
    if !bpm.is_finite() || bpm <= 0.0 {
        return Err(format!("invalid #BPM: value ({bpm})"));
    }

    struct PendingLine {
        text: String,
        word_times: Vec<f64>,
        at_word_start: bool,
        start: f64,
        /// When the latest note seen so far finishes (start + length) -
        /// the line's real sung end once every note has been read.
        end: Option<f64>,
        singer: Singer,
    }

    let mut lines: Vec<PendingLine> = Vec::new();
    let mut current: Option<PendingLine> = None;
    // No `P`-marker seen yet - `Default` rather than an explicit `Male`, so
    // a plain (non-duet) UltraStar file that never uses player markers at
    // all doesn't make the app think every line was manually assigned.
    let mut current_singer = Singer::Default;

    let finish = |current: &mut Option<PendingLine>, out: &mut Vec<PendingLine>| {
        if let Some(mut p) = current.take() {
            p.text.truncate(p.text.trim_end().len());
            if !p.text.is_empty() {
                out.push(p);
            }
        }
    };

    for raw in text.lines() {
        // Only use a trimmed copy for *structural* checks (kind marker,
        // player markers, blank/comment detection) - UltraStar syllable
        // text can have a meaningful trailing space (it's how the format
        // marks "this syllable ends a word"), so we must not trim the line
        // before extracting that text field.
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "E" {
            break;
        }
        if trimmed.eq_ignore_ascii_case("P1") {
            current_singer = Singer::Male;
            continue;
        }
        if trimmed.eq_ignore_ascii_case("P2") {
            current_singer = Singer::Female;
            continue;
        }
        if trimmed.eq_ignore_ascii_case("P3") {
            current_singer = Singer::Duet;
            continue;
        }
        if trimmed.eq_ignore_ascii_case("P4") {
            // Not part of the base UltraStar spec, but a reasonable
            // extension for a 4th performer given our own Singer enum.
            current_singer = Singer::Screaming;
            continue;
        }

        let l = raw.trim_start(); // safe to trim leading whitespace only
        let mut parts = l.splitn(5, ' ');
        let kind = parts.next().unwrap_or("");
        if !matches!(kind, ":" | "*" | "F" | "R" | "G") {
            if let Some(rest) = trimmed.strip_prefix('-') {
                // Line break: "- BEAT" (sometimes "- BEAT BEAT2"); take the
                // first number as the beat the *new* line starts at.
                let beat: Option<f64> = rest.split_whitespace().next().and_then(|s| s.parse().ok());
                if let Some(beat) = beat {
                    finish(&mut current, &mut lines);
                    let _ = ultrastar_beat_to_secs(beat, bpm, gap_ms); // next note sets the real start
                }
            }
            continue;
        }

        // ": START LENGTH PITCH TEXT" - TEXT is everything remaining after
        // the 4th space, spaces (including a trailing one) intact.
        let rest: Vec<&str> = parts.collect();
        if rest.len() < 4 {
            continue; // malformed note line - skip rather than abort the whole import
        }
        let Some(start_beat) = rest[0].parse::<f64>().ok() else {
            continue;
        };
        // rest[1] = length in beats (gives the note's end - the line's
        // real sung end is its last note's), rest[2] = pitch (parsed to
        // confirm shape, then intentionally discarded - see module docs).
        let note_end = rest[1]
            .parse::<f64>()
            .ok()
            .filter(|len| len.is_finite() && *len >= 0.0)
            .map(|len| ultrastar_beat_to_secs(start_beat + len, bpm, gap_ms));
        let _pitch: Option<i32> = rest[2].parse().ok();
        let syllable = rest[3].trim_end_matches(['\r']).to_string();
        let start_secs = ultrastar_beat_to_secs(start_beat, bpm, gap_ms);

        match &mut current {
            None => {
                current = Some(PendingLine {
                    text: syllable.clone(),
                    word_times: vec![start_secs],
                    at_word_start: syllable.ends_with(' '),
                    start: start_secs,
                    end: note_end,
                    singer: current_singer,
                });
            }
            Some(p) => {
                p.end = match (p.end, note_end) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
                // A bare `~` note just holds the previous syllable's vowel
                // on a new pitch - extend it rather than adding a unit.
                if syllable.trim() == "~" {
                    if syllable.ends_with(' ') && !p.text.ends_with(' ') {
                        // ...and if it ends the word, keep the word break.
                        p.text.push(' ');
                        p.at_word_start = true;
                    }
                    continue;
                }
                let new_word = p.at_word_start || syllable.starts_with(' ');
                if !new_word {
                    // Another syllable of the same word: its own timing
                    // unit (`beau|ti|ful`), not merged into one word.
                    p.text.push(crate::lyrics::SYLLABLE_MARK);
                }
                p.word_times.push(start_secs);
                p.text.push_str(&syllable);
                p.at_word_start = syllable.ends_with(' ') || p.text.ends_with(' ');
            }
        }
    }
    finish(&mut current, &mut lines);

    let mut result = Vec::with_capacity(lines.len());
    for (i, p) in lines.iter().enumerate() {
        let mut line = LyricLine::new(p.text.as_str());
        line.start = Some(p.start);
        line.singer = p.singer;
        // Real note data, not a guess - stored as an actual end time.
        line.sing_end_override = p.end.filter(|e| *e > p.start);
        line.starts_new_block = i == 0 || is_new_block(lines[i - 1].start, p.start);
        if p.word_times.len() == line.word_overrides.len() {
            line.word_overrides = p.word_times.iter().map(|t| Some(*t)).collect();
        }
        result.push(line);
    }
    Ok(result)
}

/// Notional BPM used to encode our own second-based timing into
/// UltraStar's beat format on export - chosen purely for beat-resolution
/// precision (37.5ms/beat at this value), not as a claim about the song's
/// actual musical tempo (this app has no tempo-detection feature). Real
/// UltraStar files are normally authored against the song's real BPM, but
/// nothing about the format *requires* that - a game only ever converts
/// beats back to seconds using the file's own declared BPM/GAP, exactly
/// the inverse of [`ultrastar_beat_to_secs`], so any fixed BPM round-trips
/// correctly as long as we're consistent about it.
const EXPORT_ULTRASTAR_BPM: f64 = 400.0;

/// Inverse of [`ultrastar_beat_to_secs`], rounded to the nearest whole beat
/// (UltraStar beats are always integers) and clamped to non-negative -
/// `gap_secs` is subtracted first so beat 0 lines up with whatever moment
/// the caller chose as the file's `#GAP:`.
fn secs_to_beat(secs: f64, gap_secs: f64, bpm: f64) -> i64 {
    (((secs - gap_secs) * bpm * 4.0 / 60.0).round() as i64).max(0)
}

fn singer_to_player_marker(singer: Singer) -> &'static str {
    match singer {
        // UltraStar has no "unassigned" concept of its own - an untouched
        // line is just the primary performer, same as an explicit `Male`.
        Singer::Default | Singer::Male => "P1",
        Singer::Female => "P2",
        Singer::Duet => "P3",
        // Not part of the base UltraStar spec, but the same reasonable
        // extension `import_ultrastar` already accepts on the way in.
        Singer::Screaming => "P4",
        // No dedicated UltraStar slot for it - the second performer is
        // the closest match for a backing voice.
        Singer::Backing => "P2",
    }
}

/// Exports timed lines to an UltraStar `.txt` note file - the inverse of
/// [`import_ultrastar`]. `mp3_filename` (a bare filename, not a path - see
/// [`crate::export::paired_audio_path`]) is written as the `#MP3:` header
/// when given.
///
/// Pitch has no equivalent in this app's data model (no melody/pitch-
/// tracking feature), so every note is written with a constant placeholder
/// pitch (`0`) - a game that scores pitch accuracy would treat every note
/// as exactly on-pitch or exactly off, but the lyrics and their timing are
/// real. Player markers (`P1`-`P4`) are only written where the singer
/// actually changes from the previous line (and never before a leading
/// `Default`/`Male` line - `Default` renders and round-trips the same as an
/// explicit `Male`), so a single-voice song exports as a plain, non-duet
/// file rather than one needlessly marked up with a redundant `P1` on every
/// line.
pub fn export_ultrastar(
    timed: &[TimedLine],
    title: Option<&str>,
    artist: Option<&str>,
    mp3_filename: Option<&str>,
) -> String {
    let bpm = EXPORT_ULTRASTAR_BPM;
    let gap_secs = timed.first().map(|l| l.start).unwrap_or(0.0);

    let mut out = String::new();
    out.push_str("#ENCODING:UTF8\n");
    out.push_str(&format!("#TITLE:{}\n", title.unwrap_or("Untitled")));
    out.push_str(&format!("#ARTIST:{}\n", artist.unwrap_or("Unknown")));
    if let Some(mp3) = mp3_filename {
        out.push_str(&format!("#MP3:{mp3}\n"));
    }
    out.push_str(&format!("#GAP:{:.0}\n", gap_secs * 1000.0));
    out.push_str(&format!("#BPM:{bpm:.2}\n"));

    let mut last_singer: Option<Singer> = None;
    for (i, line) in timed.iter().enumerate() {
        let changed = last_singer != Some(line.singer);
        let leading_unassigned_voice =
            i == 0 && matches!(line.singer, Singer::Male | Singer::Default);
        if changed && !leading_unassigned_voice {
            out.push_str(singer_to_player_marker(line.singer));
            out.push('\n');
        }
        last_singer = Some(line.singer);

        let words = crate::lyrics::word_timings(line);
        let last_word = words.len().saturating_sub(1);
        for (w, word) in words.iter().enumerate() {
            let start_beat = secs_to_beat(word.highlight_at, gap_secs, bpm);
            let end_beat = secs_to_beat(word.held_until, gap_secs, bpm);
            let length_beats = (end_beat - start_beat).max(1);
            // A trailing space on every syllable but a line's last is how
            // UltraStar marks "a word boundary follows" when syllables are
            // concatenated - see `import_ultrastar`'s own `at_word_start`.
            let suffix = if word.ends_word && w != last_word {
                " "
            } else {
                ""
            };
            out.push_str(&format!(
                ": {start_beat} {length_beats} 0 {}{suffix}\n",
                word.text
            ));
        }

        if let Some(next) = timed.get(i + 1) {
            let break_beat = secs_to_beat(next.start, gap_secs, bpm);
            out.push_str(&format!("- {break_beat}\n"));
        }
    }
    out.push_str("E\n");
    out
}

// ---------------------------------------------------------------------
// KOK
// ---------------------------------------------------------------------

/// Parses one KOK timestamp token, e.g. `"1,56000"` -> `1.56` seconds.
/// KOK uses a comma as the decimal separator (the format originates from
/// French karaoke software), not as a thousands separator.
fn parse_kok_time(tok: &str) -> Option<f64> {
    let t: f64 = tok.trim().replacen(',', ".", 1).parse().ok()?;
    // `f64::parse` accepts "nan"/"inf"/"infinity" (case-insensitive) as
    // valid floats - a bare `nan` token in a timestamp slot would
    // otherwise silently produce a NaN timestamp that poisons the
    // `sort_by` in `import_kok` below (`NaN.partial_cmp` is `None`, and
    // that sort's `.unwrap()` panics on it).
    (t.is_finite() && t >= 0.0).then_some(t)
}

/// Imports a KOK file (`digits,digits;text;` word-timestamp pairs - see
/// the module docs for where this format came from).
///
/// **Known limitation:** every KOK sample we could confirm the grammar of
/// only demonstrated word-level timing, with no distinct marker for line
/// breaks found in the documentation we could locate. Rather than guess at
/// an unconfirmed line-break convention, this groups words into display
/// lines heuristically: a new line starts whenever there's a >1.2s pause
/// since the previous word "should" have ended (estimated from its length)
/// or a line would otherwise exceed 10 words. If your KOK file's actual
/// verse structure doesn't come out right, re-group the text in the paste
/// box after importing - the word-level timing itself will still be intact
/// as long as you don't re-parse over it.
pub fn import_kok(text: &str) -> Vec<LyricLine> {
    let tokens: Vec<&str> = text.split(';').collect();
    let mut words: Vec<(f64, String)> = Vec::new();
    let mut i = 0;
    while i + 1 < tokens.len() {
        if let Some(secs) = parse_kok_time(tokens[i]) {
            let word = tokens[i + 1].trim();
            if !word.is_empty() {
                words.push((secs, word.to_string()));
            }
        }
        i += 2;
    }
    words.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    const MAX_WORDS_PER_LINE: usize = 10;
    const WORD_GAP_THRESHOLD: f64 = 1.2;

    let mut line_groups: Vec<Vec<(f64, String)>> = Vec::new();
    let mut current: Vec<(f64, String)> = Vec::new();
    for (t, w) in words {
        let should_break = match current.last() {
            Some((prev_t, prev_w)) => {
                let estimated_prev_duration = (prev_w.chars().count() as f64 * 0.09).max(0.15);
                (t - (prev_t + estimated_prev_duration) > WORD_GAP_THRESHOLD)
                    || current.len() >= MAX_WORDS_PER_LINE
            }
            None => false,
        };
        if should_break {
            line_groups.push(std::mem::take(&mut current));
        }
        current.push((t, w));
    }
    if !current.is_empty() {
        line_groups.push(current);
    }

    let mut result = Vec::with_capacity(line_groups.len());
    for (i, group) in line_groups.iter().enumerate() {
        let text = group
            .iter()
            .map(|(_, w)| w.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let start = group[0].0;
        let mut line = LyricLine::new(&text);
        line.start = Some(start);
        line.starts_new_block = i == 0 || {
            let prev_end = line_groups[i - 1].last().unwrap().0;
            is_new_block(prev_end, start)
        };
        let has_word_times = group.len() == line.word_overrides.len();
        if has_word_times {
            line.word_overrides = group.iter().map(|(t, _)| Some(*t)).collect();
        }
        let next_start = line_groups.get(i + 1).map(|g| g[0].0);
        let last_word = has_word_times
            .then(|| group.last().map(|(t, w)| (*t, w.as_str())))
            .flatten();
        line.estimated_sing_duration = estimate_imported_sing_duration(
            start,
            line.word_overrides.len(),
            last_word,
            next_start,
        );
        result.push(line);
    }
    result
}

/// Imports plain, untimed text - this app's native paste-box format.
/// Provided here too so callers importing "by format" (rather than always
/// using the paste box directly) have a consistent entry point.
pub fn import_plain_text(text: &str) -> Vec<LyricLine> {
    crate::lyrics::parse_pasted_lyrics(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_palette() -> crate::video::VideoPalette {
        let c = crate::video::Rgb8::new(200, 200, 200);
        crate::video::VideoPalette {
            background: c,
            default_unsung: c,
            default_highlight: c,
            male_unsung: c,
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
            backing_unsung: c,
            backing_highlight: c,
        }
    }

    fn subtitle_line(text: &str, start: f64, sing_end: f64, singer: Singer) -> TimedLine {
        let mut l = TimedLine::new(text.to_string(), start, sing_end + 2.0, singer);
        l.sing_end = sing_end;
        l
    }

    #[test]
    fn srt_has_numbered_cues_with_millisecond_times() {
        let lines = vec![
            subtitle_line("Hello  world", 5.25, 7.0, Singer::Default),
            subtitle_line("Again", 3725.5, 3727.0, Singer::Default),
        ];
        let srt = export_srt(&lines, Some("Song"), Some("Band"), 4.0);
        assert!(
            srt.starts_with("1\n00:00:00,000 --> 00:00:04,000\nSong\nby Band\n\n"),
            "{srt}"
        );
        assert!(
            srt.contains("2\n00:00:05,250 --> 00:00:07,500\nHello world\n\n"),
            "{srt}"
        );
        assert!(
            srt.contains("3\n01:02:05,500 --> 01:02:07,500\nAgain\n\n"),
            "{srt}"
        );
    }

    #[test]
    fn srt_without_title_starts_with_the_first_line() {
        let srt = export_srt(
            &[subtitle_line("Hi", 1.0, 2.0, Singer::Default)],
            None,
            None,
            4.0,
        );
        assert!(
            srt.starts_with("1\n00:00:01,000 --> 00:00:02,500\nHi\n"),
            "{srt}"
        );
    }

    #[test]
    fn ass_karaoke_tags_follow_word_timing() {
        let mut line = subtitle_line("one two three", 10.0, 13.0, Singer::Female);
        line.word_overrides = vec![Some(10.0), Some(11.0), Some(12.0)];
        // "two" stops filling at 11.5 - a pause before "three".
        line.word_end_overrides = vec![None, Some(11.5), None];
        let ass = export_ass(&[line], &[], &test_palette(), "Arial", None, None, 0.0);
        // Shown from 9.0 (1s lead-in): a 100cs unsung wait, then the words.
        assert!(
            ass.contains(
                "Dialogue: 0,0:00:09.00,0:00:13.50,Female,,0,0,0,karaoke,\
                 {\\k100}{\\kf100}one {\\kf50}two {\\k50}{\\kf100}three\n"
            ),
            "{ass}"
        );
        assert!(ass.contains("Style: Female,Arial,72,"));
        assert!(
            !ass.contains("Style: Male,"),
            "only voices in use get a style"
        );
    }

    #[test]
    fn ass_lead_in_never_overlaps_the_previous_main_line() {
        let a = subtitle_line("a", 1.0, 4.6, Singer::Default);
        let b = subtitle_line("b", 5.0, 6.0, Singer::Default);
        let ass = export_ass(&[a, b], &[], &test_palette(), "Arial", None, None, 0.0);
        assert!(ass.contains("Dialogue: 0,0:00:04.60,"), "{ass}");
    }

    #[test]
    fn ass_backing_lines_get_their_own_style_and_text_is_escaped() {
        let main = subtitle_line("lead", 1.0, 3.0, Singer::Male);
        let back = subtitle_line("{echo} \\o/", 2.0, 4.0, Singer::Backing);
        let ass = export_ass(
            &[main],
            &[back],
            &test_palette(),
            "My, Font",
            Some("T"),
            Some("A"),
            0.9,
        );
        assert!(ass.contains("Style: Backing lane,My  Font,54,"), "{ass}");
        assert!(ass.contains(",Backing lane,,0,0,0,karaoke,"));
        assert!(ass.contains("(echo) {\\kf67}/o/"), "{ass}");
        assert!(ass.contains("Dialogue: 0,0:00:00.00,0:00:00.90,Title,,0,0,0,,{\\pos(960,480)}T\n"));
        assert!(ass.contains(",Artist,,0,0,0,,{\\pos(960,600)}by A\n"));
    }

    #[test]
    fn outro_is_appended_to_srt_and_ass() {
        let outro = crate::export::Outro {
            start: 200.0,
            end: 210.0,
            lines: vec!["Thanks!".to_string()],
            show_title: true,
        };
        let mut srt = export_srt(
            &[subtitle_line("Hi", 1.0, 2.0, Singer::Default)],
            None,
            None,
            0.5,
        );
        append_srt_outro(&mut srt, &outro, Some("Song"), Some("Band"));
        assert!(
            srt.ends_with("2\n00:03:20,000 --> 00:03:30,000\nThanks!\nSong\nby Band\n\n"),
            "{srt}"
        );
        let mut ass = export_ass(&[], &[], &test_palette(), "Arial", None, None, 0.0);
        append_ass_outro(&mut ass, &outro, Some("Song"), None);
        assert!(ass.contains("Dialogue: 0,0:03:20.00,0:03:30.00,Title,,0,0,0,,{\\pos(960,485)\\fad(600,0)}Thanks!\n"), "{ass}");
        assert!(
            ass.contains(",Title,,0,0,0,,{\\pos(960,595)\\fad(600,0)}Song\n"),
            "{ass}"
        );
    }

    #[test]
    fn ass_colors_are_bgr_with_opaque_alpha() {
        assert_eq!(
            ass_color(crate::video::Rgb8::new(0x11, 0x22, 0x33)),
            "&H00332211"
        );
        assert_eq!(ass_time(3725.456), "1:02:05.46");
        assert_eq!(srt_time(3725.4567), "01:02:05,457");
    }

    #[test]
    fn detects_lrc() {
        let text = "[ar:Some Artist]\n[00:12.34]Hello there\n[00:15.00]General Kenobi";
        assert_eq!(detect_format(text), LyricFormat::Lrc);
    }

    #[test]
    fn detects_ultrastar() {
        let text =
            "#TITLE:Song\n#ARTIST:Someone\n#BPM:200\n#GAP:1000\n: 0 4 0 Hel\n: 4 4 0 lo \n- 8\nE";
        assert_eq!(detect_format(text), LyricFormat::UltraStar);
    }

    #[test]
    fn detects_kok() {
        let text = "1,56000;This ;2,12006;is ;2,5678;the ;3,02345;first ;3,4321;line;";
        assert_eq!(detect_format(text), LyricFormat::Kok);
    }

    #[test]
    fn detects_plain_text_as_fallback() {
        let text = "Just some\nplain lyrics\nwith no tags at all";
        assert_eq!(detect_format(text), LyricFormat::PlainText);
    }

    #[test]
    fn import_lrc_basic_line_level() {
        let text = "[ar:Someone]\n[ti:A Song]\n[00:12.00]First line\n[00:15.50]Second line";
        let lines = import_lrc(text);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "First line");
        assert_eq!(lines[0].start, Some(12.0));
        assert_eq!(lines[1].text, "Second line");
        assert_eq!(lines[1].start, Some(15.5));
    }

    #[test]
    fn import_lrc_handles_multiple_time_tags_per_line() {
        let text = "[00:10.00][01:20.00]Chorus line";
        let lines = import_lrc(text);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].start, Some(10.0));
        assert_eq!(lines[1].start, Some(80.0));
        assert_eq!(lines[0].text, "Chorus line");
    }

    #[test]
    fn import_lrc_sorts_out_of_order_timestamps() {
        let text = "[00:20.00]Second\n[00:05.00]First";
        let lines = import_lrc(text);
        assert_eq!(lines[0].text, "First");
        assert_eq!(lines[1].text, "Second");
    }

    #[test]
    fn import_lrc2_populates_word_overrides() {
        let text = "[00:12.00]<00:12.00>Some <00:12.90>words <00:13.40>here";
        let lines = import_lrc(text);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Some words here");
        assert_eq!(
            lines[0].word_overrides,
            vec![Some(12.0), Some(12.9), Some(13.4)]
        );
    }

    #[test]
    fn import_lrc_marks_new_blocks_on_big_gaps() {
        let text = "[00:01.00]a\n[00:03.00]b\n[00:20.00]c";
        let lines = import_lrc(text);
        assert!(lines[0].starts_new_block);
        assert!(!lines[1].starts_new_block);
        assert!(lines[2].starts_new_block);
    }

    #[test]
    fn export_lrc_simple_roundtrips_readable_format() {
        let mut lines = vec![LyricLine::new("Hello"), LyricLine::new("World")];
        lines[0].start = Some(12.0);
        lines[1].start = Some(15.5);
        let timed = crate::lyrics::resolve_timing(&lines, Some(20.0));
        let out = export_lrc(&timed, false, None, None);
        assert!(out.contains("[00:12.00]Hello"));
        assert!(out.contains("[00:15.50]World"));
    }

    #[test]
    fn export_lrc_enhanced_includes_word_tags() {
        let mut lines = vec![LyricLine::new("one two")];
        lines[0].start = Some(0.0);
        let timed = crate::lyrics::resolve_timing(&lines, Some(4.0));
        let out = export_lrc(&timed, true, None, None);
        assert!(out.starts_with("[00:00.00]"));
        assert!(out.contains('<'));
        assert!(out.contains("one"));
        assert!(out.contains("two"));
    }

    #[test]
    fn export_lrc_writes_title_and_artist_tags_when_given() {
        let mut lines = vec![LyricLine::new("Hello")];
        lines[0].start = Some(1.0);
        let timed = crate::lyrics::resolve_timing(&lines, Some(4.0));
        let out = export_lrc(&timed, false, Some("A Song"), Some("Someone"));
        assert!(out.starts_with("[ti:A Song]\n[ar:Someone]\n"));
    }

    #[test]
    fn export_lrc_omits_title_artist_tags_when_absent() {
        let mut lines = vec![LyricLine::new("Hello")];
        lines[0].start = Some(1.0);
        let timed = crate::lyrics::resolve_timing(&lines, Some(4.0));
        let out = export_lrc(&timed, false, None, None);
        assert!(!out.contains("[ti:"));
        assert!(!out.contains("[ar:"));
    }

    #[test]
    fn export_ultrastar_writes_expected_headers_and_terminator() {
        let mut lines = vec![LyricLine::new("Hello world")];
        lines[0].start = Some(2.0);
        let timed = crate::lyrics::resolve_timing(&lines, Some(6.0));
        let out = export_ultrastar(&timed, Some("A Song"), Some("Someone"), Some("song.mp3"));
        assert!(out.contains("#ENCODING:UTF8\n"));
        assert!(out.contains("#TITLE:A Song\n"));
        assert!(out.contains("#ARTIST:Someone\n"));
        assert!(out.contains("#MP3:song.mp3\n"));
        assert!(out.contains("#GAP:2000\n"));
        assert!(out.trim_end().ends_with('E'));
    }

    #[test]
    fn export_ultrastar_uses_placeholder_headers_when_title_artist_missing() {
        let mut lines = vec![LyricLine::new("Hello")];
        lines[0].start = Some(0.0);
        let timed = crate::lyrics::resolve_timing(&lines, Some(4.0));
        let out = export_ultrastar(&timed, None, None, None);
        assert!(out.contains("#TITLE:Untitled\n"));
        assert!(out.contains("#ARTIST:Unknown\n"));
        assert!(!out.contains("#MP3:"));
    }

    #[test]
    fn export_ultrastar_omits_player_markers_for_an_all_male_song() {
        let mut lines = vec![LyricLine::new("a"), LyricLine::new("b")];
        lines[0].start = Some(0.0);
        lines[1].start = Some(2.0);
        let timed = crate::lyrics::resolve_timing(&lines, Some(4.0));
        let out = export_ultrastar(&timed, None, None, None);
        assert!(!out.contains("P1"));
    }

    #[test]
    fn export_ultrastar_marks_singer_changes_but_not_repeats() {
        let mut lines = vec![
            LyricLine::new("a"),
            LyricLine::new("b"),
            LyricLine::new("c"),
        ];
        lines[0].start = Some(0.0);
        lines[0].singer = Singer::Female;
        lines[1].start = Some(2.0);
        lines[1].singer = Singer::Female;
        lines[2].start = Some(4.0);
        lines[2].singer = Singer::Male;
        let timed = crate::lyrics::resolve_timing(&lines, Some(6.0));
        let out = export_ultrastar(&timed, None, None, None);
        assert_eq!(out.matches("P2").count(), 1);
        assert_eq!(out.matches("P1").count(), 1);
    }

    #[test]
    fn export_ultrastar_inserts_a_linebreak_between_lines() {
        let mut lines = vec![LyricLine::new("a"), LyricLine::new("b")];
        lines[0].start = Some(0.0);
        lines[1].start = Some(2.0);
        let timed = crate::lyrics::resolve_timing(&lines, Some(4.0));
        let out = export_ultrastar(&timed, None, None, None);
        assert!(out.lines().any(|l| l.starts_with("- ")));
    }

    #[test]
    fn export_ultrastar_round_trips_through_import() {
        let mut lines = vec![LyricLine::new("hello world"), LyricLine::new("second line")];
        lines[0].start = Some(3.0);
        lines[0].word_overrides = vec![Some(3.0), Some(3.6)];
        lines[1].start = Some(8.0);
        let timed = crate::lyrics::resolve_timing(&lines, Some(12.0));

        let out = export_ultrastar(&timed, Some("T"), Some("A"), None);
        let imported = import_ultrastar(&out).unwrap();

        assert_eq!(imported.len(), 2);
        assert_eq!(imported[0].text, "hello world");
        assert_eq!(imported[1].text, "second line");
        // Beat quantization at EXPORT_ULTRASTAR_BPM introduces at most
        // ~1/2 beat (~18.75ms) of rounding error.
        const TOL: f64 = 0.02;
        assert!((imported[0].start.unwrap() - 3.0).abs() < TOL);
        assert!((imported[1].start.unwrap() - 8.0).abs() < TOL);
        assert!((imported[0].word_overrides[0].unwrap() - 3.0).abs() < TOL);
        assert!((imported[0].word_overrides[1].unwrap() - 3.6).abs() < TOL);
    }

    #[test]
    fn ultrastar_beat_conversion_matches_known_formula() {
        // 200 BPM, no gap: 1 beat = 60/(200*4) = 0.075s
        assert!((ultrastar_beat_to_secs(1.0, 200.0, 0.0) - 0.075).abs() < 1e-9);
        // With a 1000ms gap.
        assert!((ultrastar_beat_to_secs(0.0, 200.0, 1000.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn import_ultrastar_basic_two_lines() {
        let text = "#TITLE:Test\n#BPM:200\n#GAP:0\n\
                     : 0 4 0 Hel\n\
                     : 4 4 0 lo \n\
                     : 8 4 0 world\n\
                     - 12\n\
                     : 12 4 0 Sec\n\
                     : 16 4 0 ond \n\
                     : 20 4 0 line\n\
                     E";
        let lines = import_ultrastar(text).unwrap();
        assert_eq!(lines.len(), 2);
        // Each UltraStar syllable keeps its own note time as a timing unit
        // (`Hel|lo`), displayed as one word.
        assert_eq!(lines[0].text, "Hel|lo world");
        assert_eq!(lines[1].text, "Sec|ond line");
        assert_eq!(crate::lyrics::normalize_text(&lines[0].text), "Hello world");
        // beat 0 at 200bpm/gap0 -> 0.0s
        assert_eq!(lines[0].start, Some(0.0));
        assert_eq!(
            lines[0].word_overrides,
            vec![Some(0.0), Some(0.3), Some(0.6)]
        );
    }

    #[test]
    fn import_ultrastar_tilde_holds_extend_the_previous_syllable() {
        let text = "#BPM:200\n#GAP:0\n: 0 4 0 Oh\n: 4 4 0 ~\n: 8 4 0 ~ \n: 12 4 0 yeah\nE";
        let lines = import_ultrastar(text).unwrap();
        assert_eq!(lines[0].text, "Oh yeah");
        assert_eq!(lines[0].word_overrides, vec![Some(0.0), Some(0.9)]);
        // The held notes still count toward the line's real end.
        assert!((lines[0].sing_end_override.unwrap() - 1.2).abs() < 1e-9);
    }

    #[test]
    fn lrc2_tags_inside_a_word_become_syllables_and_round_trip() {
        let lines = import_lrc(
            "[00:01.00]<00:01.00>beau<00:01.50>ti<00:02.00>ful <00:03.00>day<00:04.00>\n",
        );
        assert_eq!(lines[0].text, "beau|ti|ful day");
        assert_eq!(
            lines[0].word_overrides,
            vec![Some(1.0), Some(1.5), Some(2.0), Some(3.0)]
        );
        // Exported back as LRC2: syllable tags mid-word, space only after
        // the word ends.
        let tl = TimedLine::from_line(&lines[0], 1.0, 5.0);
        let out = export_lrc(&[tl], true, None, None);
        assert!(
            out.contains("<00:01.00>beau<00:01.50>ti<00:02.00>ful <00:03.00>day "),
            "{out}"
        );
    }

    #[test]
    fn ultrastar_export_joins_syllables_and_round_trips() {
        let mut line = LyricLine::new("beau|ti|ful day");
        line.start = Some(1.0);
        line.word_overrides = vec![Some(1.0), Some(1.5), Some(2.0), Some(3.0)];
        let tl = TimedLine::from_line(&line, 1.0, 4.0);
        let out = export_ultrastar(&[tl], None, None, None);
        let back = import_ultrastar(&out).unwrap();
        assert_eq!(back[0].text, "beau|ti|ful day");
        for (a, b) in back[0].word_overrides.iter().zip(&line.word_overrides) {
            assert!((a.unwrap() - b.unwrap()).abs() < 0.04, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn ass_karaoke_tags_split_syllables_without_spaces() {
        let mut line = LyricLine::new("beau|ti|ful day");
        line.word_overrides = vec![Some(10.0), Some(10.5), Some(11.0), Some(12.0)];
        let mut tl = TimedLine::from_line(&line, 10.0, 14.0);
        tl.sing_end = 13.0;
        let ass = export_ass(&[tl], &[], &test_palette(), "Arial", None, None, 0.0);
        assert!(
            ass.contains("{\\kf50}beau{\\kf50}ti{\\kf100}ful {\\kf100}day\n"),
            "{ass}"
        );
    }

    #[test]
    fn import_ultrastar_missing_bpm_is_an_error() {
        let text = "#TITLE:Test\n: 0 4 0 Hi\nE";
        assert!(import_ultrastar(text).is_err());
    }

    #[test]
    fn import_ultrastar_rejects_nan_bpm() {
        let text = "#BPM:nan\n#GAP:0\n: 0 4 0 Hi\nE";
        let err = import_ultrastar(text).unwrap_err();
        assert!(err.contains("invalid #BPM"), "unexpected error: {err}");
    }

    #[test]
    fn import_ultrastar_treats_nan_gap_as_zero() {
        let text = "#BPM:200\n#GAP:nan\n: 0 4 0 Hi\nE";
        let lines = import_ultrastar(text).unwrap();
        assert_eq!(lines[0].start, Some(0.0));
    }

    #[test]
    fn import_ultrastar_maps_player_markers_to_singer() {
        let text = "#BPM:200\n#GAP:0\nP1\n: 0 4 0 His \n: 4 4 0 line\n- 8\nP2\n: 8 4 0 Her \n: 12 4 0 line\nE";
        let lines = import_ultrastar(text).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].singer, Singer::Male);
        assert_eq!(lines[1].singer, Singer::Female);
    }

    #[test]
    fn import_kok_basic_word_timing() {
        let text = "1,56000;This ;2,12006;is ;2,5678;the ;3,02345;first ;3,4321;line;";
        let lines = import_kok(text);
        assert!(!lines.is_empty());
        let all_words: String = lines
            .iter()
            .map(|l| l.text.clone())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(all_words.contains("This"));
        assert!(all_words.contains("first"));
        assert!(all_words.contains("line"));
        assert_eq!(lines[0].start, Some(1.56));
    }

    #[test]
    fn parse_kok_time_rejects_non_finite_and_negative_values() {
        assert_eq!(parse_kok_time("1,56000"), Some(1.56));
        assert_eq!(parse_kok_time("nan"), None);
        assert_eq!(parse_kok_time("inf"), None);
        assert_eq!(parse_kok_time("-1,0"), None);
    }

    #[test]
    fn import_kok_skips_a_word_with_a_nan_timestamp() {
        let text = "nan;bad;1,0;good;";
        let lines = import_kok(text);
        let all_words: String = lines
            .iter()
            .map(|l| l.text.clone())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!all_words.contains("bad"));
        assert!(all_words.contains("good"));
    }

    #[test]
    fn import_kok_splits_on_max_words_per_line() {
        let mut text = String::new();
        for i in 0..15 {
            text.push_str(&format!("{},00000;w{};", i, i));
        }
        let lines = import_kok(&text);
        assert!(
            lines.len() >= 2,
            "expected the 15-word stream to split into multiple lines"
        );
        for l in &lines {
            assert!(l.text.split_whitespace().count() <= 10);
        }
    }

    #[test]
    fn import_plain_text_matches_native_parser() {
        let text = "a\nb\n\nc";
        assert_eq!(import_plain_text(text).len(), 3);
    }

    // --- imported end-time estimates ---------------------------------------

    fn assert_close(actual: Option<f64>, expected: f64) {
        let a = actual.expect("expected Some estimate");
        assert!((a - expected).abs() < 1e-6, "expected {expected}, got {a}");
    }

    #[test]
    fn import_lrc1_estimates_end_just_before_next_line() {
        let lines = import_lrc("[00:10.00]one two three\n[00:13.00]four five");
        // 3s gap minus the lead, under the 3-word cap of 3s.
        assert_close(lines[0].estimated_sing_duration, 2.8);
        assert_eq!(lines[0].sing_end_override, None);
        // Last line: no next line to bound it - falls back to the default.
        assert_eq!(lines[1].estimated_sing_duration, None);
    }

    #[test]
    fn import_lrc1_estimate_is_capped_before_a_long_break() {
        let lines = import_lrc("[00:10.00]hi there\n[00:40.00]next line");
        assert_close(lines[0].estimated_sing_duration, IMPORTED_MIN_SING_SECS);
    }

    #[test]
    fn import_lrc1_blank_timed_line_marks_where_singing_stops() {
        let lines = import_lrc("[00:10.00]one two three four\n[00:12.00]\n[00:30.00]next");
        assert_eq!(lines.len(), 2);
        assert_close(lines[0].estimated_sing_duration, 1.8);
    }

    #[test]
    fn import_lrc2_estimates_end_shortly_after_the_last_word() {
        let lines = import_lrc("[00:10.00]<00:10.00>Hello <00:11.00>world\n[00:20.00]next");
        // Last word starts 1.0s in; "world" (5 chars) gets 0.5s.
        assert_close(lines[0].estimated_sing_duration, 1.5);
        assert_eq!(lines[0].sing_end_override, None);
    }

    #[test]
    fn import_lrc2_trailing_tag_is_a_real_line_end() {
        let lines =
            import_lrc("[00:10.00]<00:10.00>Hello <00:11.00>world <00:12.50>\n[00:20.00]next");
        assert_eq!(lines[0].word_overrides, vec![Some(10.0), Some(11.0)]);
        assert_eq!(lines[0].sing_end_override, Some(12.5));
        assert_eq!(lines[0].estimated_sing_duration, None);
    }

    #[test]
    fn import_ultrastar_stores_last_note_end_as_real_end() {
        let text = "#BPM:200\n#GAP:0\n\
                     : 0 4 0 Hel\n\
                     : 4 4 0 lo \n\
                     : 8 6 0 world\n\
                     - 16\n\
                     : 16 4 0 Bye\n\
                     E";
        let lines = import_ultrastar(text).unwrap();
        // Last note: beat 8 + 6 = 14 -> 14 * 60 / 800 = 1.05s.
        assert_close(lines[0].sing_end_override, 1.05);
        // beat 16 + 4 = 20 -> 1.5s.
        assert_close(lines[1].sing_end_override, 1.5);
    }

    #[test]
    fn import_kok_estimates_end_from_last_word() {
        // Two lines split by a long pause.
        let text = "1,00000;One ;1,50000;two;10,00000;Three;";
        let lines = import_kok(text);
        assert_eq!(lines.len(), 2);
        // Last word "two" starts 0.5s in, gets the 0.4s minimum.
        assert_close(lines[0].estimated_sing_duration, 0.9);
    }

    #[test]
    fn estimate_never_ends_before_the_last_word_starts() {
        // Next line starts almost immediately after the last word.
        let est = estimate_imported_sing_duration(10.0, 2, Some((11.0, "word")), Some(11.1));
        assert_close(est, 1.05);
    }
}
