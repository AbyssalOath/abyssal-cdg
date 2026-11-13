//! The export dialog's pre-export checklist: things worth a look before
//! exporting that don't stop an export (missing timing, which does, is
//! `main.rs`'s `missing_timing_reason`). Pure functions over the lyric
//! data, so every check is unit-testable without the UI.

use crate::cdg::SAFE_COLS;
use crate::font;
use crate::lyrics::{self, LyricLine, TimedLine, TimingSettings};

/// A line sung for less than this gets its whole word wipe crammed into a
/// blink - almost always a mis-tap (e.g. a double-press).
const MIN_LINE_SECS: f64 = 0.3;
/// A word highlighted for less than this flashes rather than wipes.
const MIN_WORD_SECS: f64 = 0.06;
/// How many line numbers an aggregated issue lists before "and N more".
const MAX_LISTED_LINES: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// Will visibly look wrong in the output.
    Warning,
    /// Worth knowing, often fine.
    Note,
}

/// A one-click fix the checklist can offer next to an issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fix {
    /// Split this line in two at its middle word (see `lyrics::split_line`).
    SplitLine(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub severity: Severity,
    pub message: String,
    /// The (first) line this is about, for a "Show" button.
    pub line: Option<usize>,
    pub fix: Option<Fix>,
}

/// Which outputs are about to be exported - some checks only matter to some.
#[derive(Clone, Copy, Debug, Default)]
pub struct Targets {
    pub cdg: bool,
    /// Any export that uses lyric timing (`.cdg`, video, LRC, UltraStar).
    pub timed: bool,
}

/// Runs every check relevant to `targets`, warnings first.
pub fn run(
    lines: &[LyricLine],
    title: &str,
    artist: &str,
    settings: &TimingSettings,
    duration: Option<f64>,
    targets: Targets,
) -> Vec<Issue> {
    let mut issues = Vec::new();
    if targets.cdg {
        cdg_line_length(lines, &mut issues);
        cdg_unsupported_chars(lines, title, artist, &mut issues);
    }
    if targets.timed {
        let (main, main_idx) = lyrics::resolve_timing_indexed(lines, duration, settings);
        let (backing, backing_idx) = lyrics::resolve_backing_track_indexed(lines, settings);
        let mut timed: Vec<(usize, &TimedLine)> = main_idx
            .iter()
            .copied()
            .zip(main.iter())
            .chain(backing_idx.iter().copied().zip(backing.iter()))
            .collect();
        timed.sort_by_key(|(i, _)| *i);
        past_song_end(&timed, duration, &mut issues);
        short_lines(&timed, &mut issues);
        short_words(&timed, &mut issues);
        estimated_word_timing(lines, &mut issues);
    }
    issues.sort_by_key(|i| i.severity != Severity::Warning);
    issues
}

/// "lines 3, 5, 9" / "line 3" - 1-based, capped at [`MAX_LISTED_LINES`].
fn line_list(indices: &[usize]) -> String {
    let shown: Vec<String> = indices
        .iter()
        .take(MAX_LISTED_LINES)
        .map(|i| (i + 1).to_string())
        .collect();
    let mut out = format!(
        "{} {}",
        if indices.len() == 1 { "line" } else { "lines" },
        shown.join(", ")
    );
    if indices.len() > MAX_LISTED_LINES {
        out.push_str(&format!(" and {} more", indices.len() - MAX_LISTED_LINES));
    }
    out
}

fn cdg_line_length(lines: &[LyricLine], issues: &mut Vec<Issue>) {
    for (i, line) in lines.iter().enumerate() {
        let len = lyrics::normalize_text(&line.text).chars().count();
        if len > SAFE_COLS as usize {
            issues.push(Issue {
                severity: Severity::Warning,
                message: format!(
                    "Line {} is {len} characters - a .cdg row fits {SAFE_COLS}, so the end \
                     (\"…{}\") gets cut off.",
                    i + 1,
                    lyrics::normalize_text(&line.text)
                        .chars()
                        .skip(SAFE_COLS as usize)
                        .collect::<String>()
                ),
                line: Some(i),
                fix: lyrics::best_split_word(&line.text).map(|_| Fix::SplitLine(i)),
            });
        }
    }
}

fn cdg_unsupported_chars(lines: &[LyricLine], title: &str, artist: &str, issues: &mut Vec<Issue>) {
    const MAX_LISTED_CHARS: usize = 12;
    let mut chars: Vec<char> = Vec::new();
    let mut first_line = None;
    let mut collect = |text: &str, line: Option<usize>| {
        for ch in font::cdg_unsupported_chars(text) {
            if !chars.contains(&ch) {
                chars.push(ch);
            }
            first_line = first_line.or(line);
        }
    };
    collect(title, None);
    collect(artist, None);
    for (i, line) in lines.iter().enumerate() {
        collect(&line.text, Some(i));
    }
    if chars.is_empty() {
        return;
    }
    let mut listed = chars
        .iter()
        .take(MAX_LISTED_CHARS)
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    if chars.len() > MAX_LISTED_CHARS {
        listed.push_str(&format!(" (+{} more)", chars.len() - MAX_LISTED_CHARS));
    }
    let where_ = match first_line {
        Some(i) => format!(", first on line {}", i + 1),
        None => " (in the title/artist)".to_string(),
    };
    issues.push(Issue {
        severity: Severity::Warning,
        message: format!(
            "The .cdg's built-in font can't draw: {listed}{where_}. They'll show as blank \
             spaces in the .cdg (the video export uses a real font and isn't affected)."
        ),
        line: first_line,
        fix: None,
    });
}

fn past_song_end(timed: &[(usize, &TimedLine)], duration: Option<f64>, issues: &mut Vec<Issue>) {
    let Some(d) = duration else { return };
    let late: Vec<usize> = timed
        .iter()
        .filter(|(_, t)| t.start >= d)
        .map(|(i, _)| *i)
        .collect();
    if let Some(&first) = late.first() {
        issues.push(Issue {
            severity: Severity::Warning,
            message: format!(
                "{} start{} after the song ends, so {} never shown.",
                capitalize(&line_list(&late)),
                if late.len() == 1 { "s" } else { "" },
                if late.len() == 1 { "it's" } else { "they're" }
            ),
            line: Some(first),
            fix: None,
        });
    }
}

fn short_lines(timed: &[(usize, &TimedLine)], issues: &mut Vec<Issue>) {
    for (i, t) in timed {
        let secs = t.sing_end - t.start;
        if secs < MIN_LINE_SECS {
            issues.push(Issue {
                severity: Severity::Warning,
                message: format!(
                    "Line {} is only sung for {secs:.2}s, so its whole wipe flashes by - \
                     probably a double-tap. Re-tap it or fix its end time.",
                    i + 1
                ),
                line: Some(*i),
                fix: None,
            });
        }
    }
}

fn short_words(timed: &[(usize, &TimedLine)], issues: &mut Vec<Issue>) {
    let flagged: Vec<usize> = timed
        .iter()
        // A line already flagged as too short is covered by that warning.
        .filter(|(_, t)| t.sing_end - t.start >= MIN_LINE_SECS)
        .filter(|(_, t)| {
            lyrics::word_timings(t)
                .iter()
                .any(|w| w.held_until - w.highlight_at < MIN_WORD_SECS)
        })
        .map(|(i, _)| *i)
        .collect();
    if let Some(&first) = flagged.first() {
        issues.push(Issue {
            severity: Severity::Note,
            message: format!(
                "{} {} a word highlighted for under {:.0}ms, which flashes rather than \
                 wipes - check its word timing in the fine-tune panel.",
                capitalize(&line_list(&flagged)),
                if flagged.len() == 1 { "has" } else { "have" },
                MIN_WORD_SECS * 1000.0
            ),
            line: Some(first),
            fix: None,
        });
    }
}

/// Indices of timed lines whose word timing is still entirely the even
/// estimate - multi-word lines with no word after the first set by hand
/// (or by auto-align / an import). Also drives the step bar's
/// "Fine-tune" step in `main.rs`.
pub fn lines_on_estimated_timing(lines: &[LyricLine]) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, l)| {
            l.start.is_some()
                && l.words().len() > 1
                && l.word_overrides.iter().skip(1).all(Option::is_none)
        })
        .map(|(i, _)| i)
        .collect()
}

fn estimated_word_timing(lines: &[LyricLine], issues: &mut Vec<Issue>) {
    let estimated = lines_on_estimated_timing(lines);
    if let Some(&first) = estimated.first() {
        issues.push(Issue {
            severity: Severity::Note,
            message: format!(
                "{} still {} estimated word timing (an even guess across the line). Fine \
                 for a quick export; for an accurate wipe, click words in the fine-tune \
                 panel or run Auto-align.",
                capitalize(&line_list(&estimated)),
                if estimated.len() == 1 { "uses" } else { "use" }
            ),
            line: Some(first),
            fix: None,
        });
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timed(text: &str, start: f64, end: f64) -> LyricLine {
        let mut l = LyricLine::new(text);
        l.start = Some(start);
        l.sing_end_override = Some(end);
        l
    }

    fn all() -> Targets {
        Targets {
            cdg: true,
            timed: true,
        }
    }

    fn check(lines: &[LyricLine], duration: Option<f64>) -> Vec<Issue> {
        run(lines, "", "", &TimingSettings::default(), duration, all())
    }

    #[test]
    fn clean_song_has_no_warnings() {
        let mut a = timed("hello there", 1.0, 3.0);
        a.word_overrides = vec![Some(1.0), Some(2.0)];
        let issues = check(&[a], Some(60.0));
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn long_line_is_flagged_for_cdg_with_a_split_fix() {
        let long = "word ".repeat(12); // 59 chars normalized
        let issues = check(&[timed(&long, 1.0, 5.0)], None);
        let issue = issues
            .iter()
            .find(|i| i.fix.is_some())
            .expect("long line issue");
        assert_eq!(issue.fix, Some(Fix::SplitLine(0)));
        assert_eq!(issue.severity, Severity::Warning);
        // Not a CDG export - no length warning.
        let issues = run(
            &[timed(&long, 1.0, 5.0)],
            "",
            "",
            &TimingSettings::default(),
            None,
            Targets {
                cdg: false,
                timed: true,
            },
        );
        assert!(issues.iter().all(|i| i.fix.is_none()));
    }

    #[test]
    fn exactly_48_chars_is_fine() {
        let text = format!("{} {}", "a".repeat(23), "b".repeat(24));
        assert_eq!(text.chars().count(), 48);
        let issues = check(&[timed(&text, 1.0, 5.0)], None);
        assert!(issues.iter().all(|i| i.fix.is_none()));
    }

    #[test]
    fn undrawable_chars_are_one_warning() {
        let issues = check(&[timed("愛 してる", 1.0, 3.0)], None);
        assert!(issues
            .iter()
            .any(|i| i.message.contains("can't draw") && i.line == Some(0)));
    }

    #[test]
    fn double_tapped_line_is_flagged() {
        let issues = check(&[timed("oops", 1.0, 1.1)], None);
        assert!(issues
            .iter()
            .any(|i| i.severity == Severity::Warning && i.message.contains("0.10s")));
    }

    #[test]
    fn line_after_song_end_is_flagged() {
        let issues = check(&[timed("late", 70.0, 72.0)], Some(60.0));
        assert!(issues
            .iter()
            .any(|i| i.message.contains("after the song ends")));
    }

    #[test]
    fn estimated_word_timing_is_a_note_listing_lines() {
        let lines = vec![
            timed("a b", 1.0, 2.0),
            timed("c d", 3.0, 4.0),
            timed("single", 5.0, 6.0),
        ];
        let issues = check(&lines, None);
        let note = issues
            .iter()
            .find(|i| i.message.contains("estimated word timing"))
            .expect("note");
        assert_eq!(note.severity, Severity::Note);
        assert!(note.message.starts_with("Lines 1, 2 "), "{}", note.message);
    }

    #[test]
    fn crowded_word_is_a_note() {
        let mut l = timed("a b c", 1.0, 3.0);
        l.word_overrides = vec![Some(1.0), Some(2.0), Some(2.02)];
        let issues = check(&[l], None);
        assert!(issues.iter().any(|i| i.message.contains("under 60ms")));
    }

    #[test]
    fn warnings_sort_before_notes() {
        let lines = vec![timed("a b", 1.0, 2.0), timed("oops", 3.0, 3.05)];
        let issues = check(&lines, None);
        let first_note = issues.iter().position(|i| i.severity == Severity::Note);
        let last_warning = issues.iter().rposition(|i| i.severity == Severity::Warning);
        assert!(last_warning < first_note);
    }

    #[test]
    fn line_list_caps_and_counts_the_rest() {
        let many: Vec<usize> = (0..13).collect();
        assert_eq!(
            line_list(&many),
            "lines 1, 2, 3, 4, 5, 6, 7, 8, 9, 10 and 3 more"
        );
        assert_eq!(line_list(&[4]), "line 5");
    }
}
