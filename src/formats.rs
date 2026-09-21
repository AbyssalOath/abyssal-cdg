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

    entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    let mut result = Vec::with_capacity(entries.len());
    for (i, (start, content)) in entries.iter().enumerate() {
        let (text_only, word_times) = parse_lrc2_inline_tags(content);
        let trimmed = text_only.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut line = LyricLine::new(trimmed);
        line.start = Some(*start);
        line.starts_new_block = i == 0 || is_new_block(entries[i - 1].0, *start);
        if word_times.len() == line.word_overrides.len() {
            line.word_overrides = word_times.into_iter().map(Some).collect();
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
                out.push_str(&format!("<{wm:02}:{ws:05.2}>{} ", w.text));
            }
            out.push('\n');
        } else {
            out.push_str(&line.text);
            out.push('\n');
        }
    }
    out
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
            gap_ms = v.trim().replace(',', ".").parse().unwrap_or(0.0);
        }
    }
    let bpm =
        bpm.ok_or_else(|| "missing #BPM: header - not a recognizable UltraStar file".to_string())?;
    if bpm <= 0.0 {
        return Err(format!("invalid #BPM: value ({bpm})"));
    }

    struct PendingLine {
        text: String,
        word_times: Vec<f64>,
        at_word_start: bool,
        start: f64,
        singer: Singer,
    }

    let mut lines: Vec<(f64, String, Vec<f64>, Singer)> = Vec::new();
    let mut current: Option<PendingLine> = None;
    // No `P`-marker seen yet - `Default` rather than an explicit `Male`, so
    // a plain (non-duet) UltraStar file that never uses player markers at
    // all doesn't make the app think every line was manually assigned.
    let mut current_singer = Singer::Default;

    let finish = |current: &mut Option<PendingLine>,
                  out: &mut Vec<(f64, String, Vec<f64>, Singer)>| {
        if let Some(p) = current.take() {
            let trimmed = p.text.trim_end().to_string();
            if !trimmed.is_empty() {
                out.push((p.start, trimmed, p.word_times, p.singer));
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
        // rest[1] = length (unused), rest[2] = pitch (parsed to confirm
        // shape, then intentionally discarded - see module docs).
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
                    singer: current_singer,
                });
            }
            Some(p) => {
                if p.at_word_start {
                    p.word_times.push(start_secs);
                }
                p.text.push_str(&syllable);
                p.at_word_start = syllable.ends_with(' ') || p.text.ends_with(' ');
            }
        }
    }
    finish(&mut current, &mut lines);

    let mut result = Vec::with_capacity(lines.len());
    for (i, (start, text, word_times, singer)) in lines.iter().enumerate() {
        let mut line = LyricLine::new(text.as_str());
        line.start = Some(*start);
        line.singer = *singer;
        line.starts_new_block = i == 0 || is_new_block(lines[i - 1].0, *start);
        if word_times.len() == line.word_overrides.len() {
            line.word_overrides = word_times.iter().map(|t| Some(*t)).collect();
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
    match singer.render_as() {
        Singer::Male => "P1",
        Singer::Female => "P2",
        Singer::Duet => "P3",
        // Not part of the base UltraStar spec, but the same reasonable
        // extension `import_ultrastar` already accepts on the way in.
        Singer::Screaming => "P4",
        Singer::Default => unreachable!("render_as() never returns Default"),
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
            let suffix = if w == last_word { "" } else { " " };
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
    tok.trim().replacen(',', ".", 1).parse().ok()
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
    words.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

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
        if group.len() == line.word_overrides.len() {
            line.word_overrides = group.iter().map(|(t, _)| Some(*t)).collect();
        }
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
        assert_eq!(lines[0].text, "Hello world");
        assert_eq!(lines[1].text, "Second line");
        // beat 0 at 200bpm/gap0 -> 0.0s
        assert_eq!(lines[0].start, Some(0.0));
        // "Hello" (Hel+lo ) and "world" -> 2 words
        assert_eq!(lines[0].word_overrides.len(), 2);
        assert!(lines[0].word_overrides[0].is_some());
    }

    #[test]
    fn import_ultrastar_missing_bpm_is_an_error() {
        let text = "#TITLE:Test\n: 0 4 0 Hi\nE";
        assert!(import_ultrastar(text).is_err());
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
}
