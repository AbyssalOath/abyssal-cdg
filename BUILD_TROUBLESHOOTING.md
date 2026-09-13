# Build troubleshooting

## "It fails to compile" / dependency resolution errors

This project, as configured, **will fail to compile on any Rust toolchain older than
1.85** (released Feb 2025) - this is a dependency issue, not a bug in this project's own
code.

`eframe`/`egui` 0.28 pull in `egui-winit`, which unconditionally depends on
`smithay-clipboard` -> `smithay-client-toolkit ^0.20` -> `wayland-protocols`. The
published patch of `wayland-protocols` in that range requires Cargo's `edition2024`
feature, which only stabilized in Rust 1.85. If your `rustc`/`cargo` is older than that
(common - e.g. Ubuntu 24.04's `apt` package ships rustc 1.75, Debian stable is often
older still, and some `rustup` installs go stale), `cargo build` fails immediately with
an error like:

```
error: failed to download replaced source registry `crates-io`
Caused by:
  failed to parse manifest at `.../wayland-protocols-0.32.13/Cargo.toml`
Caused by:
  feature `edition2024` is required
```

**Fix - the simple path:** update your Rust toolchain.

```bash
rustup update stable
rustc --version   # confirm 1.85 or newer
cargo build --release
```

If you installed Rust via your OS package manager instead of `rustup`, switch to
[rustup](https://rustup.rs) - distro packages lag behind, and this project (like most
current `egui`-based apps) expects a reasonably current stable toolchain.

**Fix - if you can't upgrade Rust:** pin `eframe`/`egui` to 0.27 and drop the `wayland`
feature (X11-only fallback). This combination is known to resolve and compile cleanly on
Rust 1.75:

```toml
eframe = { version = "0.27", default-features = false, features = ["glow", "default_fonts", "persistence", "x11"] }
egui = "0.27"
```

## Verifying core logic without a display or audio device

`main.rs` and `audio.rs`'s device initialization are the only code that touches
`eframe`/`egui`/`rodio` directly - `cdg.rs`, `lyrics.rs`, `font.rs`, `export.rs`,
`formats.rs`, and `video.rs` are pure logic with no GUI/audio dependency, and are fully
covered by the unit test suite:

```bash
cargo test
```

This covers CDG packet encoding/timing, word-timing math, the countdown-window logic,
duet/screaming color resolution, lyric-file import/export (LRC, UltraStar, KOK), video
block-splitting, and the playback clock's play/pause/resume/seek state machine - all
without needing real audio hardware or a display.

If `cargo build`/`cargo test` fails with a *different* error than the one above, please
open an issue with the exact output, your `rustc --version`, and your OS.
