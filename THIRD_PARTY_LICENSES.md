# Third-party licenses

Abyssal CDG Creator's own source code is licensed under the GNU Affero
General Public License v3.0 (see [`LICENSE`](LICENSE)). This file lists
the third-party software this project bundles, links against, or
downloads on the user's behalf, and the license each is under.

Every bundled/downloaded binary and model listed here was chosen
specifically to be compatible with unrestricted redistribution alongside
this project - none require a paid license, none are "nonfree"/patent-
encumbered in a way that would need a separate license from this project
or its users, and the one weak-copyleft component (LAME) is used exactly
the way its license anticipates (a shared library the app links against,
not code copied into this project's own source).

## Bundled binaries and models

These are fetched or built by the release workflow
(`.github/workflows/release.yml`) and bundled into every installer - see
`ARCHITECTURE.md`'s "Bundled binaries and models" section for how each is
located at runtime.

### ffmpeg

- **License:** GNU Lesser General Public License v2.1 or later (LGPL).
  This project builds `ffmpeg` from unmodified upstream source
  (`scripts/build-ffmpeg.sh`) with `--disable-gpl` and `--disable-nonfree`,
  which excludes every GPL-only or patent-restricted optional component
  (most notably libx264/libx265) - confirmed directly against the actual
  build's own `./configure` output ("License: LGPL version 2.1 or later"),
  not assumed from ffmpeg's defaults.
- **Source:** <https://ffmpeg.org> / <https://github.com/FFmpeg/FFmpeg>
- **Modifications:** none - built from an unmodified release tag with a
  restricted `./configure` flag set only.

### openh264 (linked into this project's ffmpeg build)

- **License:** BSD 2-Clause, for the source and for Cisco's own official
  binary redistribution.
- **Source:** <https://github.com/cisco/openh264>
- **Note on the patent-royalty coverage:** Cisco's AVC/H.264 patent
  license coverage (see
  <https://www.openh264.org/BINARY_LICENSE.txt>) applies only to Cisco's
  own separately-distributed binary, not a third party's recompiled copy -
  confirmed by reading that license text directly. This project's `ffmpeg`
  build links against openh264's headers/ABI at build time (a copy
  compiled and discarded during the build, never distributed - see
  `scripts/build-ffmpeg.sh`), but the actual `libopenh264` shared library
  bundled with the app is Cisco's own official binary for the exact same
  version (`ffmpeg_path.rs`), downloaded from
  <http://ciscobinary.openh264.org/> - preserving that patent coverage,
  the same approach Firefox and Chromium use for the same reason.

### LAME (statically linked into this project's ffmpeg build, for MP3 encoding)

- **License:** GNU Library General Public License v2 (the LGPL's original
  name), confirmed directly against LAME's own `COPYING` file.
- **Source:** <https://lame.sourceforge.io>
- **Note:** MP3's own patents expired worldwide by 2017, so unlike H.264
  there's no separate patent-royalty question for MP3 encoding/decoding.

### ONNX Runtime

- **License:** MIT.
- **Source:** <https://github.com/microsoft/onnxruntime>
- **Note:** loaded as a shared library at runtime (`ort`'s `load-dynamic`
  feature - see `onnxrt.rs`), built from unmodified upstream source only
  for the one target (macOS x86_64) that has no official prebuilt binary;
  every other target uses Microsoft's own official release binary
  unmodified.

### UVR-MDX-NET-Inst_HQ_3 (vocal separation model weights)

- **License:** MIT, per the Ultimate Vocal Remover GUI project's own
  README (<https://github.com/Anjok07/ultimatevocalremovergui>), which
  states explicitly: *"For all third-party application developers who wish
  to use our models, please honor the MIT license by providing credit to
  UVR and its developers."* This notice serves as that credit.
- **Source:** <https://github.com/TRvlvr/model_repo> (the model file
  itself); trained/published by the Ultimate Vocal Remover project
  (Anjok07 and contributors).
- **Note:** this project's own inference code (`mdx.rs`/`stft.rs`) is an
  independent, from-scratch Rust implementation of the MDX-Net
  architecture, verified bit-faithful against `audio-separator`'s
  (MIT-licensed) reference implementation by reading its source directly -
  see `mdx.rs`'s module docs.

### Forced-alignment models (auto-align, 9 languages)

- **License:** Apache License 2.0, for every one of the 9 bundled
  languages' base checkpoints.
- **Base model sources:**
  - English: `jonatasgrosman/wav2vec2-large-xlsr-53-english`
  - Spanish/French/German/Italian/Portuguese/Japanese/Chinese:
    `jonatasgrosman/wav2vec2-large-xlsr-53-<language>`
  - Korean: `kresnik/wav2vec2-large-xlsr-korean`
    (all on <https://huggingface.co>)
- **ONNX conversion sources** (same weights, re-exported to ONNX format -
  verified byte-for-byte against each base model's own vocabulary before
  use, not merely trusted - see `align.rs`'s module docs for how):
  - English: `Xenova/wav2vec2-large-xlsr-53-english`
  - The other 8 languages: `FinDIT-Studio/wav2vec2-large-xlsr-53-<language>-onnx`
- **Note:** `MMS_FA`/`facebook/mms-1b-all`, a much broader multilingual
  alignment model, was deliberately **not** used - it's licensed
  CC-BY-NC-4.0 (non-commercial only), incompatible with unrestricted
  redistribution.

## Rust crate dependencies

Every crate this project depends on (transitively, `--avoid-dev-deps
--avoid-build-deps`) is permissively licensed - MIT, Apache-2.0,
BSD-2/3-Clause, Zlib, ISC, BSL-1.0, CC0-1.0, Unicode-3.0, or CDLA-Permissive-2.0
(all compatible with unrestricted redistribution), plus MPL-2.0 for the
`symphonia` family (weak copyleft at the *file* level - using it as an
unmodified library dependency, as this project does, doesn't extend any
obligation to this project's own source). No dependency in the tree is
GPL/AGPL-only or otherwise restrictively licensed; the one dependency
offering LGPL as an option (`r-efi`) offers it alongside MIT/Apache-2.0
alternatives, so no LGPL obligation is actually incurred.

This list was generated with [`cargo-license`](https://crates.io/crates/cargo-license)
(`cargo license --avoid-dev-deps --avoid-build-deps`) and reflects
`Cargo.lock` at the time it was last regenerated - re-run that command
after a dependency update to check for changes, rather than trusting this
file to stay current on its own.

<!-- Regenerate below with: cargo license --avoid-dev-deps --avoid-build-deps -->

```
Apache-2.0 (15): ab_glyph, ab_glyph_rasterizer, codespan-reporting, cpal, gethostname, glutin, glutin_egl_sys, glutin_glx_sys, glutin_wgl_sys, hound, oboe, oboe-sys, owned_ttf_parser, spirv, winit
Apache-2.0 AND ISC (1): ring
Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT (7): linux-raw-sys, linux-raw-sys, rustix, rustix, wasi, wasip2, wit-bindgen
Apache-2.0 OR BSD-2-Clause OR MIT (3): mach2, zerocopy, zerocopy-derive
Apache-2.0 OR BSD-3-Clause (2): moxcms, pxfm
Apache-2.0 OR BSD-3-Clause OR MIT (2): num_enum, num_enum_derive
Apache-2.0 OR CC0-1.0 (1): imgref
Apache-2.0 OR ISC OR MIT (1): rustls
Apache-2.0 OR LGPL-2.1-or-later OR MIT (2): r-efi, r-efi
Apache-2.0 OR MIT (305): (see `cargo license` for the full list - too long to usefully inline; includes ort, ndarray, rustfft, realfft, rubato, hound, bzip2-rs, ureq, dirs, unicode-normalization, serde, and most of the rest of this project's direct dependencies)
Apache-2.0 OR MIT OR Zlib (18): bytemuck, bytemuck_derive, cursor-icon, dispatch2, glow, miniz_oxide, objc2-app-kit, objc2-core-foundation, objc2-core-graphics, objc2-io-surface, raw-window-handle, raw-window-handle, tinyvec, visibility, xkeysym, zune-core, zune-inflate, zune-jpeg
BSD-2-Clause (3): av1-grain, rav1e, v_frame
BSD-3-Clause (5): avif-serialize, exr, lebe, ravif, subtle
BSL-1.0 (2): clipboard-win, error-code
CC0-1.0 (1): hexf-parse
CDLA-Permissive-2.0 (1): webpki-roots
ISC (5): libloading, libloading, libloading, rustls-webpki, untrusted
MIT (117): (see `cargo license` for the full list - includes rfd, font8x8, realfft, tracing, and most GUI/windowing-adjacent crates)
MIT OR Unlicense (7): byteorder, byteorder-lite, memchr, same-file, termcolor, walkdir, winapi-util
MPL-2.0 (18): dwrote, option-ext, symphonia, symphonia-bundle-flac, symphonia-bundle-mp3, symphonia-codec-aac, symphonia-codec-adpcm, symphonia-codec-alac, symphonia-codec-pcm, symphonia-codec-vorbis, symphonia-core, symphonia-format-caf, symphonia-format-isomp4, symphonia-format-mkv, symphonia-format-ogg, symphonia-format-riff, symphonia-metadata, symphonia-utils-xiph
Unicode-3.0 (18): icu_collections, icu_locale_core, icu_normalizer, icu_normalizer_data, icu_properties, icu_properties_data, icu_provider, litemap, potential_utf, tinystr, writeable, yoke, yoke-derive, zerofrom, zerofrom-derive, zerotrie, zerovec, zerovec-derive
Zlib (2): foldhash, slotmap
```

The DejaVu Sans / DejaVu Sans Bold fonts bundled for video export are
under the permissive Bitstream Vera license - see
`assets/fonts/dejavu/DEJAVU-LICENSE.txt`.

Two more fonts are bundled as selectable lyric-text fonts (see `fonts.rs`'s
`BUNDLED_FONTS`) - both under the permissive SIL Open Font License 1.1,
which allows commercial use, redistribution, and embedding freely (the
same license class as DejaVu's own Bitstream Vera above):
- **Creepster** - Copyright 2011 Font Diner, Inc. - see
  `assets/fonts/creepster/OFL.txt`.
- **Nosifer** - Copyright 2011 Typomondo - see
  `assets/fonts/nosifer/OFL.txt`. Used as the "Abyssal" color preset's own
  default font.

An earlier "Bloodlust" font (Iconian Fonts) was considered for this exact
spot and dropped before ever being released - its own license is
non-commercial-use-only, which doesn't fit this project's bundled-font bar
(everything baked into the compiled binary needs a license that's fine
with commercial use/redistribution, since this app itself is - see
`CONTRIBUTING.md`'s "only bundles permissively-licensed dependencies").
