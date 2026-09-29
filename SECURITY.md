# Security policy

## Supported versions

Abyssal CDG Creator is pre-1.0 and has no long-term-supported releases yet - security
fixes are made against the latest version on the default branch. If you're not on the
latest release, please update and confirm the issue still reproduces before reporting.

## Reporting a vulnerability

Please **do not** open a public GitHub issue for a security vulnerability. Instead, use
GitHub's private vulnerability reporting for this repository: go to the **Security**
tab -> **Advisories** -> **Report a vulnerability**. That reaches maintainers directly
without publicly disclosing details before a fix is available.

Please include:
- What you found and why it's a security issue (not just a crash - see "what counts"
  below).
- Steps to reproduce, and, if applicable, a minimal sample file (audio/lyrics) that
  triggers it.
- The version/commit you tested against and your OS.

There's no formal SLA (this is a small, unfunded project), but reports will get a
best-effort response and credit in the fix's changelog entry, unless you'd prefer to
stay anonymous.

## Scope and threat model

Abyssal CDG Creator is a local, single-user desktop application. It has no network
client of its own, no telemetry, no accounts, and no server component - everything it
does happens on the machine it's running on, on files the user explicitly chooses (via
a file-open/save dialog, dragging a file onto the window, or picking one from the
"Recent" list), plus a small, fixed-location autosave/recent-files store in the app's
own per-user data directory (see "Design notes" below). That significantly limits
what a "vulnerability" can mean here.

**In scope:**
- A malformed/malicious **audio file** (via `symphonia`/`rodio`), **lyrics file**
  (LRC/UltraStar/KOK, parsed by `formats.rs`), or **project file** (`.abyzl`, JSON
  parsed by `project.rs`) that causes memory corruption, or that achieves anything
  beyond a crash/panic of the app itself (e.g. arbitrary code execution, writing
  outside the user-chosen output path).
- Command injection via the `ffmpeg` subprocess (`video.rs`, and `vocals.rs` when
  writing a non-`.wav` stem) - e.g. a song title/artist, file path, or lyric content
  that escapes the intended argument boundaries. (As written, arguments are passed
  individually via `Command::arg`, not through a shell, so this shouldn't be
  reachable - but a regression here would be a real, in-scope report.)
- Anything in `model_assets.rs`'s model-download path, reused by `ffmpeg_path.rs` for
  openh264 (SSRF via a malicious URL, path traversal via the destination filename,
  TOCTOU on the temp-file rename, or - specific to openh264's `.bz2` download - a
  decompression-bomb/malformed-archive issue in the `bzip2-rs` decoder) - this is real
  network code in this app's own dependency tree (see the note below). Every download
  (openh264, both MDX vocal-isolation models, all 9 forced-alignment models) is over
  HTTPS and checked against a hash pinned in this app's own source right after
  downloading, before the file is ever loaded/`dlopen`'d - a mismatch deletes the file
  rather than caching it, so a compromised/MITM'd/tricked-CDN download can't silently
  get reused on a later run. A file already sitting in the bundled-resource or cache
  location is *not* re-hashed on every use (see `model_assets::resolve_model`'s own
  docs for why) - a still-in-scope report is one that shows this can be exploited
  despite that, e.g. by getting a bad file into the cache some other way. For vocal
  removal's model, `ffmpeg`, and openh264, this path is only a dev-build fallback (a
  packaged release bundles all three); for auto-align it's the *normal* path even in a
  packaged release, since the 9 alignment models (~1.2GB each) aren't bundled - only
  the language actually selected downloads, on first use. `onnxrt.rs` (the ONNX
  Runtime library itself) and `ffmpeg_path.rs`'s `ffmpeg` binary specifically (as
  opposed to openh264) deliberately do *not* auto-download - see the note below.
- Path traversal via a shared `.abyzl` project file's `title` field, which
  auto-fills the export dialog's base filename with no validation of its own -
  `main.rs`'s `sanitize_export_base_name` reduces it to `Path::file_name()` (discarding
  any traversal sequence or absolute-path override) before it's used to build any of
  the 6 export output paths. A report showing a way around that reduction is in scope.
- Path handling bugs that write or read outside of what the user selected (in a file
  dialog, dropped onto the window, or picked from the "Recent" list), including the
  throwaway temp files `vocals.rs`/`main.rs` create (under the OS temp dir, named with
  just the current process ID - e.g. `abyssal-cdg-align-vocals-<pid>.wav`,
  `abyssal-cdg-instrumental-<pid>.wav` - not a per-call-unique name, so two sequential
  calls in the same process reuse the same path; each is removed after use and this
  app never makes concurrent calls that could collide on one, so it's not a real
  vulnerability, just worth knowing if you add a new one of these), the per-user cache
  directories `model_assets.rs`/`ffmpeg_path.rs` download into, and the app's own
  per-user data directory (`eframe::storage_dir`) used for the crash-recovery autosave
  and the recent-files list.
- Anything in `mdx.rs`/`stft.rs`/`ctc.rs` that could turn a malformed/adversarial
  audio file (or, for `ctc.rs`, adversarial lyric text) into memory corruption
  (buffer over-/under-read in the manual chunking/indexing/trellis math) - this is
  hand-written DSP/DP code processing untrusted input, not a mature upstream library,
  so it carries real risk a thin wrapper around one wouldn't.

**Out of scope (please still file a normal bug report for these, just not as a
security issue):**
- A malformed file causing a plain crash/panic with no further consequence - this is a
  local single-user app, so a self-inflicted DoS (open a bad file, app closes) isn't
  treated as a security vulnerability, though it's still a bug worth fixing.
- Issues in `ffmpeg`, `libopenh264`, or LAME themselves - `ffmpeg` is this project's own
  custom build (`scripts/build-ffmpeg.sh`) but from unmodified upstream source, and
  `libopenh264`/the bundled/downloaded ONNX models are fixed, pinned binaries this
  project ships or fetches from a fixed URL, not something it executes as code beyond
  ordinary codec/inference use - or in decoder/runtime crates (`symphonia`, `ort`)
  upstream of this project - report those to the respective upstream project. If you're
  not sure whether a given crash is in this project's code or a dependency, report it
  here anyway and let a maintainer sort out where it belongs.
- The exported `.cdg`/`.mp4`/`.lrc`/UltraStar `.txt` files themselves being
  "insecure" in some way once handed to third-party playback software - this
  project's responsibility ends at producing a spec-conformant file.

## Design notes relevant to security

- **Network access in this app's own code**: this project's own dependency tree
  includes an HTTP client (`ureq`), used from two places in the app's own code -
  `model_assets.rs`'s `download_to_file` (the shared downloader, also used by
  `ffmpeg_path.rs` for openh264 - see below) - always to a fixed, hardcoded URL, never
  a user-supplied one. For `MdxModel::InstHq3` (the model `vocals.rs`'s
  `load_separator` always uses for the export checkboxes/video's "Remove vocals") and
  openh264, this only runs as a dev-build fallback - a packaged release bundles both
  (fetched/built by CI before packaging - see `.github/workflows/release.yml`), so a
  normal install never triggers a download for either. Auto-align's 9 language models
  are too large to bundle the same way (~1.2GB each vs. InstHq3's ~65MB), so for
  those, downloading on first use *is* the normal packaged-release behavior, not just
  a dev fallback - the first time a given language is used, expect a real, visible
  network request. `MdxModel::KimVocal2` (auto-align's own selectable alternative
  vocal-isolation model, ~65MB - see mdx.rs's `MdxModel` docs) behaves the same way as
  the language models, not InstHq3: it downloads on first use in every build,
  packaged or not, only if a user actually picks it from auto-align's "Vocal model"
  dropdown. Either way, downloads land in a per-user cache directory and are reused
  after that. Every one of these URLs is HTTPS, and every downloaded file (openh264
  plus all 11 ML models) is checked against a SHA-256 hash pinned in this app's own
  source (`model_assets::verify_sha256`) before it's trusted - a mismatch deletes the
  file instead of caching it. If a future change adds network access anywhere else
  (e.g. an update checker), it should be called out explicitly in the changelog and
  this file updated accordingly.
- The ONNX Runtime shared library itself (distinct from either ML model file) is
  loaded via `ort`'s `load-dynamic` feature (`onnxrt.rs`), not linked in at build
  time - deliberately, since ONNX Runtime publishes no prebuilt binary at all for one
  of this app's four release targets (macOS x86_64), so that target's release build
  needs a from-source build instead of a normal download-and-link. Unlike the model
  files, `onnxrt.rs` does *not* auto-download a fallback copy for a dev build missing
  one - it comes packed inside a multi-file archive rather than being one file on its
  own, so a dev build with none bundled just fails with a clear error containing exact
  manual setup instructions (the official archive URL to fetch, or - for macOS
  x86_64, which has no official archive - a pointer to the release workflow's
  from-source build job) rather than silently fetching and unpacking an archive.
- `ffmpeg` is invoked as a subprocess with a fixed, hardcoded argument list plus a
  handful of typed values (paths, an enum-selected resolution, an integer fps) passed
  as individual `Command::arg`s - never interpolated into a shell string. Keep it that
  way; if you add a new argument, add it as its own `.arg(...)` call. `ffmpeg` itself
  is a binary this project builds from source and bundles (`ffmpeg_path.rs`), not a
  system install anymore - see "Bundled binaries and models" in ARCHITECTURE.md for
  the LGPL-only/openh264/LAME configuration.
- Vocal removal (`vocals.rs`/`mdx.rs`) and auto-align (`align.rs`/`ctc.rs`) both run
  their ONNX model in-process via `ort` (ONNX Runtime) - no subprocess, no separately
  installed tool for either. Neither shells out to Python/`audio-separator`/`aeneas`
  anymore.
- File I/O is mostly user-directed: audio/lyrics/project files are opened, and
  `.cdg`/`.mp4`/instrumental audio/vocals audio/`.lrc`/UltraStar `.txt`/`.abyzl`
  project files are saved, through `rfd`'s native file dialogs, a file dropped onto
  the window, or a path picked from the "Recent" list (itself built only from paths
  the user previously opened through one of those routes). The paths this app takes
  on its own initiative are the crash-recovery autosave and the recent-files list
  (written to a fixed location in the OS's own per-user application-data directory,
  `eframe::storage_dir` - the same mechanism `eframe` already uses for
  window-position persistence) and the model/binary cache directories
  `model_assets.rs`/`ffmpeg_path.rs` download into (always for auto-align's language
  models and `MdxModel::KimVocal2`; a dev-build-only fallback for `MdxModel::InstHq3`,
  ffmpeg, and openh264) - never a location the user didn't implicitly consent to just
  by running the app. This
  is in addition to the throwaway temp files/directories `video.rs`/`vocals.rs` create
  for an in-progress operation and delete afterward.
- **Release artifact integrity**: every GitHub release publishes a `SHA256SUMS.txt`
  (generated by `.github/workflows/release.yml`'s `release` job) alongside the
  installers, so a download can be verified against what was actually built. This is
  *not* code signing - it proves a download matches this project's own build, not who
  built it - and this project has neither a paid Windows code-signing certificate nor
  a paid Apple Developer ID/notarization, so Windows SmartScreen and macOS Gatekeeper
  will both warn on a fresh install regardless. Every action `release.yml`/`ci.yml`
  runs is pinned to a specific commit SHA (not a mutable version tag), so a compromised
  or retargeted upstream Action tag can't silently change what CI executes.
