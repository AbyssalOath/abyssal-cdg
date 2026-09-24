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
//! happen again). A "Getting Started" article written specifically for
//! in-app reading, and genuinely new "Internals" articles (code-level
//! execution-flow walkthroughs nothing else covers), are deliberately left
//! for later - see this module's own doc history/PHASES-style planning,
//! not built speculatively now.

/// One Codex article - `content` is the *entire* embedded file, rendered
/// as-is via `egui_commonmark`, not excerpted or reformatted for in-app
/// display.
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
        content: include_str!("../DISCLAIMER.md"),
    },
    Article {
        title: "Changelog",
        category: "Release Notes",
        content: include_str!("../CHANGELOG.md"),
    },
];
