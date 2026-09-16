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
does happens on the machine it's running on, on files the user explicitly chooses via
a file-open/save dialog. That significantly limits what a "vulnerability" can mean here.

**In scope:**
- A malformed/malicious **audio file** (via `symphonia`/`rodio`) or **lyrics file**
  (LRC/UltraStar/KOK, parsed by `formats.rs`) that causes memory corruption, or that
  achieves anything beyond a crash/panic of the app itself (e.g. arbitrary code
  execution, writing outside the user-chosen output path).
- Command injection via the `ffmpeg` subprocess (`video.rs`) or the `audio-separator`
  subprocess (`vocals.rs`) - e.g. a song title/artist, file path, or lyric content
  that escapes the intended argument boundaries. (As written, arguments are passed
  individually via `Command::arg`, not through a shell, so this shouldn't be reachable
  - but a regression here would be a real, in-scope report.)
- Path handling bugs that write or read outside of what the user selected in a file
  dialog, including `vocals.rs`'s throwaway temp directory (created under the OS temp
  dir with a per-process/per-call unique name, and removed after use).

**Out of scope (please still file a normal bug report for these, just not as a
security issue):**
- A malformed file causing a plain crash/panic with no further consequence - this is a
  local single-user app, so a self-inflicted DoS (open a bad file, app closes) isn't
  treated as a security vulnerability, though it's still a bug worth fixing.
- Issues in `ffmpeg` or `audio-separator` themselves (both external tools the user
  installs separately, not bundled with this app), or in decoder crates (`symphonia`)
  upstream of this project - report those to the respective upstream project. This
  includes whatever `audio-separator` itself does over the network to download its
  separation model (see below) - this project only invokes its CLI and doesn't control
  that behavior. If you're not sure whether a given crash is in this project's code or
  a dependency, report it here anyway and let a maintainer sort out where it belongs.
- The exported `.cdg`/`.mp4` files themselves being "insecure" in some way once handed
  to third-party playback software - this project's responsibility ends at producing a
  spec-conformant file.

## Design notes relevant to security

- No network access *in this app's own code*: there is no HTTP/socket client in this
  codebase's Rust dependency tree, and this project's own code never makes a network
  request. If a future change adds one directly (e.g. an update checker), it should be
  called out explicitly in the changelog and this file updated accordingly. Note the
  caveat below, though: vocal removal shells out to `audio-separator`, which does its
  own network access (downloading its model file) outside this app's control.
- `ffmpeg` and `audio-separator` are each invoked as a subprocess with a fixed,
  hardcoded argument list plus a handful of typed values (paths, an enum-selected
  resolution, an integer fps, a JSON string built from a single hardcoded model name)
  passed as individual `Command::arg`s - never interpolated into a shell string. Keep
  it that way; if you add a new argument, add it as its own `.arg(...)` call.
- Vocal removal (`vocals.rs`) runs `audio-separator` - a separate Python package the
  user installs themselves, not bundled with this app - which downloads its
  separation model file from its own model repository the first time it's used. This
  app has no control over and doesn't vet that download; it only invokes the CLI and
  reads back the resulting audio file from a temp directory it created and cleans up
  itself.
- File I/O is user-directed: audio/lyrics are opened, and `.cdg`/`.mp4`/instrumental
  audio are saved, through `rfd`'s native file dialogs. There is no path this app
  takes on its own initiative to read or write a file the user didn't pick, aside from
  the throwaway temp files/directories `video.rs`/`vocals.rs` create for an
  in-progress export and delete afterward.
