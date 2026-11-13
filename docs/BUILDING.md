# Building from source

You only need this if you want to build or change the app yourself - to
just *use* it, grab an installer from the
[latest release](https://github.com/AbyssalOath/abyssal-cdg/releases/latest)
(see [INSTALLING.md](INSTALLING.md)).

## 1. Rust

A normal, reasonably current Rust toolchain - install via
[rustup](https://rustup.rs) if you don't have one. `rustc --version` should
be **1.88 or newer** (ideally current stable); that floor comes from the
strictest MSRV among this project's dependencies (`image`/`libloading`).
**On macOS, use Rust 1.99 for now** - Rust 1.100+ can't compile an old
macOS-only dependency of the GUI toolkit (`objc2` 0.4.1); CI and releases
are pinned to 1.99 too. If `cargo build` fails outright, see
[BUILD_TROUBLESHOOTING.md](../BUILD_TROUBLESHOOTING.md) - it covers this
and an old toolchain, by far the most common cause.

## 2. System packages

**Linux** needs the GTK3 and X11/Wayland development headers that `rfd`
(file dialogs) and `winit` (windowing) build against, plus ALSA's
development headers for `rodio`/`cpal` (audio playback). Most desktops
already have the ALSA *runtime*, but the `-dev`/`-devel` package (with the
`alsa.pc` pkg-config file) is a separate install - its absence is the most
common "why won't this build" surprise on a fresh machine or a minimal CI
container:

```bash
# Debian/Ubuntu
sudo apt install libgtk-3-dev libxkbcommon-dev libx11-dev libasound2-dev pkg-config

# Fedora
sudo dnf install gtk3-devel libxkbcommon-devel libX11-devel alsa-lib-devel

# Arch
sudo pacman -S gtk3 libxkbcommon libx11 alsa-lib
```

**macOS and Windows** need no extra system packages - just the Xcode
Command Line Tools / MSVC build tools that `cargo` already expects.

## 3. Build and run

From the repository root:

```bash
cargo build --release
cargo run --release
```

The first build takes a couple of minutes (it pulls in a GUI toolkit and
an audio decoder); later builds are fast.

## ffmpeg, ONNX Runtime and the ML models

None of these are *build*-time dependencies - `cargo build` and `cargo
test` need none of them and make no network requests.

- **ffmpeg** (MP4 video export, MP3 output, the MP3+G zip's MP3
  conversion). An installed release bundles its own custom-built ffmpeg.
  A `cargo run` dev build has no bundle, so it uses a cached copy (see
  below) or falls back to a system `ffmpeg` on your `PATH`:

  ```bash
  # Debian/Ubuntu
  sudo apt install ffmpeg
  # macOS
  brew install ffmpeg
  # Windows: download from https://ffmpeg.org/download.html and add it to PATH
  ```

  The `.cdg`, LRC/UltraStar, subtitle and WAV exports don't need ffmpeg at
  all. The app checks for it before a video export and says clearly if
  it's missing, rather than failing silently.
- **Vocal removal and auto-align** run their ONNX models in-process (no
  Python, no subprocess). A dev build downloads ONNX Runtime and the
  separation model into a per-user cache on first use; auto-align's
  per-language models always download on first use (~1.2GB each, then
  cached).

[ARCHITECTURE.md](../ARCHITECTURE.md#bundled-binaries-and-models-no-external-subprocess-dependencies-left)
explains exactly how each of these is built, bundled and located, and why.

## Running the tests

```bash
cargo test
```

The test suite needs no display, audio device, network, ffmpeg or ML
models. It covers the timing math, the `.cdg` encoder (decoded back and
checked frame by frame), the importers/exporters, the video renderer's
drawing, and the app's own pure logic. A few heavier integration tests
(real ML models, a real ffmpeg) are `#[ignore]`d by default - see their doc
comments for how to run them. CI also runs:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets
```

## Where to start reading

[ARCHITECTURE.md](../ARCHITECTURE.md) is the module map and data flow;
[LEARN_THE_CODE.md](LEARN_THE_CODE.md) is the long, code-level walkthrough
(also built into the app's Codex). [CONTRIBUTING.md](../CONTRIBUTING.md)
covers conventions and how to propose changes.
