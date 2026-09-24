# Build troubleshooting

## "It fails to compile" / dependency resolution errors

This project, as configured (via the committed `Cargo.lock`), **will fail to compile on
any Rust toolchain older than 1.88** - this is a dependency issue, not a bug in this
project's own code. That floor is the strictest `rust-version` declared by any locked
dependency (`image`/`libloading`, both 1.88 - re-check with `cargo metadata --format-version
1 | jq -r '.packages[] | select(.rust_version) | "\(.rust_version) \(.name)"' | sort -rV |
head` if a future dependency bump changes it) - a toolchain below that fails outright
with an error like:

```
error: package `image v0.25.10` cannot be built because it requires rustc 1.88.0 or newer,
while the currently active rustc version is 1.85.0
```

**If your toolchain is even older (below 1.85, released Feb 2025)**, you'll hit a
different, more confusing error first: `eframe`/`egui` 0.28 pull in `egui-winit`, which
unconditionally depends on `smithay-clipboard` -> `smithay-client-toolkit ^0.20` ->
`wayland-protocols`. The published patch of `wayland-protocols` in that range requires
Cargo's `edition2024` feature, which only stabilized in Rust 1.85:

```
error: failed to download replaced source registry `crates-io`
Caused by:
  failed to parse manifest at `.../wayland-protocols-0.32.13/Cargo.toml`
Caused by:
  feature `edition2024` is required
```

Either way, the fix is the same:

```bash
rustup update stable
rustc --version   # confirm 1.88 or newer
cargo build --release
```

(Common on Ubuntu 24.04's `apt` package, which ships rustc 1.75; Debian stable is often
older still; some `rustup` installs go stale too.) If you installed Rust via your OS
package manager instead of `rustup`, switch to [rustup](https://rustup.rs) - distro
packages lag behind, and this project (like most current `egui`-based apps) expects a
reasonably current stable toolchain.

## Verifying core logic without a display or audio device

`main.rs` and `audio.rs`'s device initialization are the only code that touches
`eframe`/`egui`/`rodio` directly - `cdg.rs`, `lyrics.rs`, `font.rs`, `export.rs`,
`formats.rs`, `video.rs`, `timeline.rs`, `project.rs`, `recent.rs`, `waveform.rs`,
`align.rs`, `ctc.rs`, `vocals.rs`, `mdx.rs`, `stft.rs`, `model_assets.rs`, and
`onnxrt.rs` are pure logic with no GUI dependency, and are fully covered by the
unit test suite:

```bash
cargo test
```

This covers CDG packet encoding/timing, word-timing math (including per-word start/end
overrides), the countdown-window and sung-line-blanking logic, timecode format/parse
and start/end overlap validation, duet/screaming color resolution, lyric-file
import/export (LRC, UltraStar, KOK), video block-splitting, the fine-tuning timeline's
zoom/drag math, the vocal-separation STFT/ISTFT round-trip math (`stft.rs`) and its
chunking constants (`mdx.rs`), the CTC forced-alignment trellis and bundled-vocab
sanity checks (`ctc.rs`/`align.rs`), bundled/cached resource path resolution
(`model_assets.rs`/`onnxrt.rs` - see ARCHITECTURE.md's "Bundled binaries and models"),
WAV round-tripping and missing-file handling (`vocals.rs`), project save/load and the
crash-recovery autosave, the recent-files list, and the playback clock's
play/pause/resume/seek state machine - all without needing real audio hardware, a
display, or `ffmpeg` installed, and without needing the actual (not committed to the
repo) ffmpeg binary, openh264 library, or ONNX model files either (those are only
needed to actually *run* video export/auto-align/vocal removal, not to build or test
the project).

If `cargo build`/`cargo test` fails with a *different* error than the one above, please
open an issue with the exact output, your `rustc --version`, and your OS.
