# Contributing

Thanks for considering a contribution to Abyssal CDG Creator. This is a small,
focused desktop app, so the bar for a good change is usually "does it work, is it
tested where that's practical, and does it match the surrounding style" rather than
anything heavyweight.

## Getting set up

See the README's [Building](README.md#building) section for toolchain/OS package
requirements. Short version:

```bash
cargo build --release
cargo test      # runs the whole pure-logic test suite, no display/audio needed
cargo run --release
```

If `cargo build` fails outright, check
[BUILD_TROUBLESHOOTING.md](BUILD_TROUBLESHOOTING.md) first - the most common cause is
an old Rust toolchain, not a bug in this project.

## Before opening a PR

- `cargo test` passes.
- `cargo clippy --all-targets` doesn't introduce *new* warnings (some pre-existing ones,
  like `too_many_arguments` on the video-frame drawing helpers, are known and accepted -
  no need to go fix unrelated ones as part of your change).
- If you touched `cdg.rs`, `lyrics.rs`, `font.rs`, `export.rs`, `formats.rs`,
  `video.rs`, `timeline.rs`, or `vocals.rs`'s output-resolution logic, add or update a
  unit test alongside the change - these modules are pure logic with no GUI/audio
  dependency, so there's no excuse not to. `main.rs` (GUI wiring, including the
  timeline widget's painting/interaction code) has no automated tests; changes there
  are verified by running the app. The actual `ffmpeg`/`audio-separator` subprocess
  invocations aren't exercised in the test suite either, since they need the external
  tool installed - verify those manually.
- If your change affects `export.rs` (`.cdg`), `video.rs` (`.mp4`), the live preview,
  or the fine-tuning timeline in `main.rs`, make sure they still agree - see
  [ARCHITECTURE.md](ARCHITECTURE.md#data-flow) for why that matters and which shared
  functions in `lyrics.rs` are supposed to be the single source of truth for timing
  decisions. The timeline in particular is also a *writer*, not just a display - a
  drag needs to write back the same `LyricLine` fields (`start`, `sing_end_override`,
  `word_overrides`, `word_end_overrides`) that tapping and manual entry use, and go
  through the same overlap validation where applicable.
- Update [CHANGELOG.md](CHANGELOG.md) under `## [Unreleased]` (add that heading at the
  top if it doesn't exist yet).

## Code style

- Follow the existing style in the file you're editing over any generic Rust style
  guide - this codebase leans on doc comments (`///`/`//!`) that explain *why*
  something is the way it is (a hidden constraint, a format quirk, a past bug), not
  *what* the code does line-by-line.
- Use a hyphen (`-` or `--`) instead of an em dash (`—`) in comments and docs.
- Don't add abstractions, config flags, or error handling for cases that can't happen
  in this codebase (e.g. no network calls, no untrusted multi-user input) - see the
  module docs for the assumptions each one already makes.
- Prefer a small, focused PR over a large one that bundles unrelated cleanup with a
  real fix/feature.

## Adding a new lyric-file format

`formats.rs` documents exactly why `.kbp` (KaraokeBuilder Studio) and "PowerKaraoke"
aren't implemented: this project would rather not guess at an unconfirmed grammar and
silently produce wrong timing. If you want to add support for either (or any other
format), the most useful thing you can attach to the PR is a real sample file (or a
link to the format's actual spec) to verify the parser against - a guess dressed up as
a parser is worse than no parser.

## Reporting bugs

Open a GitHub issue with:
- What you did, what you expected, what happened instead.
- The `abyssal-cdg` version (or commit) and your OS.
- For a crash or a bad export: the exact lyrics/settings that trigger it, if you can
  narrow it down - CDG/video export bugs are much easier to fix from a repro than a
  description.

For a security vulnerability, please don't open a public issue - see
[SECURITY.md](SECURITY.md) instead.
