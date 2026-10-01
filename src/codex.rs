//! In-app documentation browser ("Codex") - a `CommonMarkViewer`-rendered
//! (see `main.rs`'s `draw_codex`) window over a small static table of
//! articles, embedded into the binary at compile time via `include_str!`,
//! the same "small, git-tracked content baked into the binary" pattern
//! `fonts.rs`'s `BUNDLED_FONTS` and `mdx.rs`'s `MdxModel` table already
//! use - not `cargo-packager`'s `resources` mechanism, which this project
//! reserves for the large, not-checked-into-git binaries (ML models,
//! ffmpeg, ONNX Runtime) fetched by the release workflow just before
//! packaging. Embedding sidesteps that whole class of "resource not found
//! at its expected runtime path" bug entirely (see `model_assets.rs`'s own
//! docs for a real example that cost real CI time to track down) - there's
//! no runtime path resolution to get wrong when the content is compiled
//! directly into the executable.
//!
//! Every article here is deliberately a *rendering* of one of this
//! project's own existing root-level docs, not a separate copy - so there
//! is exactly one place each one's content is maintained, and the Codex
//! can never drift out of sync with them the way a hand-duplicated second
//! copy eventually would (this project has already paid real, repeated
//! cost this session keeping ARCHITECTURE.md/FEATURES.md/SECURITY.md/etc.
//! in sync with the code they describe - a second, separate copy purely
//! for in-app display would just be another place for the same drift to
//! happen again).
//!
//! The "Learn the Code" chapters (see [`all_articles`]/[`LEARN_THE_CODE_TEXT`])
//! follow the exact same one-file-of-record principle, with one twist:
//! `docs/LEARN_THE_CODE.md` is long enough (a code-level, file-by-file
//! walkthrough, not a short reference) that showing it as a *single*
//! article would mean scrolling through the whole thing to reach any one
//! chapter - `egui_commonmark` 0.17 has no anchor-link support to jump
//! within a rendered article. So that one file is split into several
//! `Article`s at startup (one per `##` heading), still with zero separate
//! copies on disk.

/// `DISCLAIMER.md`'s content, embedded once here and reused both as the
/// "Legal Disclaimer" Codex article below and as `main.rs`'s first-run
/// disclaimer modal's content (`draw_disclaimer_modal`) - one `include_str!`
/// site, not a second copy pointing at the same file.
pub const DISCLAIMER_TEXT: &str = include_str!("../DISCLAIMER.md");

/// One Codex article - `content` is the *entire* embedded file, rendered
/// as-is via `egui_commonmark`, not excerpted or reformatted for in-app
/// display. `Clone`/`Copy` are trivial (every field is a `&'static str`)
/// and only needed so [`all_articles`] can build a combined `Vec` out of
/// [`ARTICLES`] plus the split-apart chapters of `LEARN_THE_CODE_TEXT`.
#[derive(Clone, Copy)]
pub struct Article {
    pub title: &'static str,
    /// Which sidebar group this article is listed under - matches
    /// multiple articles onto the same category freely (none do yet,
    /// since every article today is a whole standalone doc, but a future
    /// split-up "Internals" category will).
    pub category: &'static str,
    pub content: &'static str,
}

/// Every Codex article, in the order they're listed within their category.
/// A plain static table, not a directory scan - keeps this list explicit
/// and reviewable (the same reasoning behind every other "small curated
/// set, not an open picker" choice in this codebase, e.g. `mdx.rs`'s
/// `MdxModel`) rather than silently picking up whatever happens to be
/// sitting in a folder.
pub const ARTICLES: &[Article] = &[
    Article {
        title: "Features",
        category: "Reference",
        content: include_str!("../FEATURES.md"),
    },
    Article {
        title: "Architecture",
        category: "Architecture",
        content: include_str!("../ARCHITECTURE.md"),
    },
    Article {
        title: "Build Troubleshooting",
        category: "Troubleshooting",
        content: include_str!("../BUILD_TROUBLESHOOTING.md"),
    },
    Article {
        title: "Legal Disclaimer & User Responsibility",
        category: "Legal & Content Responsibility",
        content: DISCLAIMER_TEXT,
    },
    Article {
        title: "Changelog",
        category: "Release Notes",
        content: include_str!("../CHANGELOG.md"),
    },
];

/// `docs/LEARN_THE_CODE.md`'s content, embedded once here and split into
/// several Codex articles by [`all_articles`] - see that function's own
/// docs for why this one doc becomes *several* articles rather than one
/// (unlike every other article above), while still being exactly one file
/// on disk, readable as a normal Markdown doc on GitHub.
pub const LEARN_THE_CODE_TEXT: &str = include_str!("../docs/LEARN_THE_CODE.md");

/// Sidebar category every chapter split out of [`LEARN_THE_CODE_TEXT`] is
/// listed under.
const LEARN_THE_CODE_CATEGORY: &str = "Learn the Code";

/// Byte offset of the start of every line in `md` that begins with `"## "`
/// (a level-2 Markdown heading, used as this doc's chapter-boundary
/// convention - its own single level-1 `# Learn the Code` title at the top
/// is deliberately *not* a boundary, so the intro text stays attached to
/// the first real chapter instead of becoming its own empty one).
fn heading_line_starts(md: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut pos = 0usize;
    for line in md.split_inclusive('\n') {
        if line.starts_with("## ") {
            starts.push(pos);
        }
        pos += line.len();
    }
    starts
}

/// Splits `md` (expected to be [`LEARN_THE_CODE_TEXT`], but kept generic
/// over any `&'static str` so this is testable without depending on the
/// real file's current content) into one [`Article`] per `"## "` heading,
/// each titled from that heading's own text - plus, if there's any
/// non-blank content before the first heading (the doc's title/intro), one
/// more article titled "Overview" for it. This is *why* the doc has one
/// `#` title but many `##` chapters: `egui_commonmark` 0.17 has no
/// anchor-link/jump-to-heading support, so a guide this size as a single
/// article would mean scrolling through the whole thing to reach any one
/// chapter - splitting it, while keeping exactly one file on disk, gets a
/// real chapter sidebar instead, the same "one real place this content
/// lives" guarantee every other article already has.
fn split_markdown_chapters(md: &'static str) -> Vec<Article> {
    let starts = heading_line_starts(md);
    let mut articles = Vec::new();

    let intro_end = starts.first().copied().unwrap_or(md.len());
    let intro = &md[..intro_end];
    if !intro.trim().is_empty() {
        articles.push(Article {
            title: "Overview",
            category: LEARN_THE_CODE_CATEGORY,
            content: intro,
        });
    }

    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(md.len());
        let chunk = &md[start..end];
        let heading_line_end = chunk.find('\n').unwrap_or(chunk.len());
        let Some(title) = chunk.get(3..heading_line_end).map(str::trim) else {
            continue;
        };
        if title.is_empty() {
            continue;
        }
        articles.push(Article {
            title,
            category: LEARN_THE_CODE_CATEGORY,
            content: chunk,
        });
    }

    articles
}

/// Every Codex article shown in the UI: the small curated [`ARTICLES`]
/// table, plus [`LEARN_THE_CODE_TEXT`] split into one chapter per article
/// (see [`split_markdown_chapters`]) - built once (the splitting itself is
/// cheap, but there's no reason to redo it every frame `draw_codex` runs
/// while the window is open) and cached for the life of the process.
pub fn all_articles() -> &'static [Article] {
    static COMBINED: std::sync::OnceLock<Vec<Article>> = std::sync::OnceLock::new();
    COMBINED.get_or_init(|| {
        let mut all = ARTICLES.to_vec();
        all.extend(split_markdown_chapters(LEARN_THE_CODE_TEXT));
        all
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_markdown_chapters_separates_intro_from_headed_chapters() {
        let md =
            "# Title\n\nSome intro text.\n\n## First\n\nFirst body.\n\n## Second\n\nSecond body.\n";
        let chapters = split_markdown_chapters(md);
        assert_eq!(chapters.len(), 3);
        assert_eq!(chapters[0].title, "Overview");
        assert!(chapters[0].content.contains("Some intro text."));
        assert_eq!(chapters[1].title, "First");
        assert!(chapters[1].content.contains("First body."));
        assert!(!chapters[1].content.contains("Second body."));
        assert_eq!(chapters[2].title, "Second");
        assert!(chapters[2].content.contains("Second body."));
        for c in &chapters {
            assert_eq!(c.category, LEARN_THE_CODE_CATEGORY);
        }
    }

    #[test]
    fn split_markdown_chapters_omits_overview_when_there_is_no_intro() {
        let md = "## Only Chapter\n\nBody text.\n";
        let chapters = split_markdown_chapters(md);
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].title, "Only Chapter");
    }

    #[test]
    fn split_markdown_chapters_does_not_split_on_deeper_headings() {
        let md = "## Chapter\n\nIntro.\n\n### Not A Chapter Boundary\n\nStill part of Chapter.\n";
        let chapters = split_markdown_chapters(md);
        assert_eq!(chapters.len(), 1);
        assert!(chapters[0].content.contains("Not A Chapter Boundary"));
    }

    #[test]
    fn the_real_learn_the_code_doc_splits_into_at_least_the_phase_one_chapters() {
        let chapters = split_markdown_chapters(LEARN_THE_CODE_TEXT);
        let titles: Vec<&str> = chapters.iter().map(|a| a.title).collect();
        assert!(titles.contains(&"Overview"));
        assert!(titles.contains(&"Big Picture"));
        assert!(titles.contains(&"Project Map"));
        assert!(titles.contains(&"Glossary"));
    }

    #[test]
    fn all_articles_includes_both_the_static_table_and_learn_the_code_chapters() {
        let all = all_articles();
        assert!(all.iter().any(|a| a.title == "Features"));
        assert!(all
            .iter()
            .any(|a| a.category == LEARN_THE_CODE_CATEGORY && a.title == "Big Picture"));
    }
}
