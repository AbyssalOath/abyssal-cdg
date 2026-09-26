#!/usr/bin/env bash
# Builds this app's bundled ffmpeg from source: LGPL-only (no x264/x265),
# with H.264 encoding via openh264 (linked at build time for the headers/
# ABI only - the actual runtime library is a separately-downloaded copy of
# Cisco's own official binary, not this build's; see ffmpeg_path.rs for
# why), MP3 encoding via LAME (statically linked - LGPL, and MP3's patents
# expired in 2017, so no equivalent runtime-swap dance is needed), and AV1
# decoding via dav1d (statically linked - BSD-2-Clause, VideoLAN's own
# software AV1 decoder). dav1d specifically (not just "AV1 support") is
# load-bearing: ffmpeg's own built-in "av1" decoder has no real software
# fallback in this ffmpeg version - every pixel format it can produce is
# gated behind a `#if CONFIG_AV1_*_HWACCEL` (VAAPI/NVDEC/VULKAN/...), and
# with none of those hwaccels compiled in (this build passes
# --disable-autodetect below, which disables all of them at configure
# time - see that flag's own comment), it unconditionally fails every AV1
# frame with "Your platform doesn't support hardware accelerated AV1
# decoding" (libavcodec/av1dec.c's own `get_pixel_format`) - regardless of
# the actual machine's GPU, and regardless of any `-hwaccel` CLI flag,
# since the problem is a missing decode path, not a hwaccel negotiation
# preference. dav1d is a completely separate registered decoder
# ("libdav1d", see libavcodec/libdav1d.c) that doesn't go through that
# hwaccel-only code at all, and - once --enable-libdav1d is set below -
# ffmpeg's own decoder lookup already prefers it over the native "av1"
# decoder automatically for every AV1 stream (`libdav1d_decoder` is
# registered before `av1_decoder` in libavcodec/allcodecs.c, and decoder
# lookup returns the first registered match for a given codec ID) - no
# `-c:v` override needed in src/video.rs, and nothing here needs to touch
# how non-AV1 inputs (the common case) get decoded.
#
# AV1 *encoding* (the video export's own output codec - see `VideoCodec`
# in src/video.rs) is via SVT-AV1 (statically linked - BSD-2-Clause-
# Patent, Alliance for Open Media/Intel/Netflix's own AV1 encoder,
# license-compatible with this project the same way dav1d is above).
# Unlike openh264's H.264, it has a real quality-targeted (CRF) mode, and
# AV1 itself is meaningfully more efficient than H.264 per bit - this app
# switched to it as the default export codec after measuring that
# openh264 alone (even after aggressively tuning its bitrate down) had no
# path to file sizes competitive with real-world karaoke video, since
# openh264 has no CRF equivalent at all (see `VideoCodec::H264`'s own
# docs). H.264 is kept selectable, not removed, purely for playback
# compatibility with anything that can't decode AV1 yet.
#
# Used identically across all 4 release targets (see
# .github/workflows/release.yml) - platform differences are handled by the
# environment variables below, not by forking this script. The Linux
# recipe (default env, no cross-compile args) was validated by actually
# running it end-to-end; the macOS cross-compile and Windows/MSYS2 paths
# are written from ffmpeg's/openh264's own documented flags but haven't
# been run for real outside this project's own CI - see the release
# workflow's comments.
#
# Required env vars:
#   OPENH264_ARCH      - x86_64 or arm64
#   OUT_DIR            - where to copy the final ffmpeg binary
# Optional env vars:
#   OPENH264_OS        - openh264's own OS name (linux, darwin, mingw_nt,
#                         ...). Auto-detected by default, using the *exact*
#                         same `uname`-based formula openh264's own
#                         Makefile uses internally (`uname | tr A-Z a-z |
#                         tr -d '-0-9.' | ...`) - guessing this string
#                         instead (e.g. "msys" for a Windows/MSYS2 build)
#                         is what broke the first real CI run of this
#                         script: MSYS2's `uname` actually reports
#                         something like "MINGW64_NT-10.0-XXXXX", which
#                         that formula reduces to "mingw_nt", not "msys".
#                         Only override this if auto-detection is ever
#                         wrong for a new environment.
#   FFMPEG_CONFIGURE_EXTRA - extra ./configure flags (e.g. cross-compile
#                             flags for macOS x86_64-on-arm64, or
#                             --target-os=mingw32 for Windows)
#   FFMPEG_EXE_NAME    - output filename (default: ffmpeg, or ffmpeg.exe
#                         if FFMPEG_CONFIGURE_EXTRA mentions mingw)
#   LAME_CONFIGURE_EXTRA - extra ./configure flags for LAME (e.g.
#                           --host=x86_64-apple-darwin when
#                           cross-compiling - LAME's own autotools build
#                           needs this independently of ffmpeg's
#                           cross-compile flags, since it's a completely
#                           separate build)
#   CC                 - overrides the C compiler LAME's build uses (e.g.
#                         "clang -arch x86_64" for the same cross case) -
#                         picked up automatically by LAME's autotools
#                         `configure`; ffmpeg's own cross-compile flags are
#                         set independently via FFMPEG_CONFIGURE_EXTRA
#   SVT_AV1_CMAKE_EXTRA - extra `cmake` configure args for SVT-AV1 (e.g.
#                          -DCMAKE_OSX_ARCHITECTURES=x86_64 for the macOS
#                          x86_64-on-arm64 cross case - CMake's own native
#                          way to target a different arch on macOS, unlike
#                          dav1d's Meson build above, which needs a whole
#                          cross file for the same case)

set -euo pipefail

: "${OPENH264_ARCH:?}"
: "${OUT_DIR:?}"
SVT_AV1_CMAKE_EXTRA="${SVT_AV1_CMAKE_EXTRA:-}"
FFMPEG_CONFIGURE_EXTRA="${FFMPEG_CONFIGURE_EXTRA:-}"
LAME_CONFIGURE_EXTRA="${LAME_CONFIGURE_EXTRA:-}"
# Exactly openh264's own Makefile's OS-detection formula (see the comment
# above) - not guessed independently, so it can't drift out of sync with
# whatever that Makefile actually expects.
OPENH264_OS="${OPENH264_OS:-$(uname | tr 'A-Z' 'a-z' | tr -d -- '-0-9.' | sed -E 's/^(net|open|free)bsd/bsd/')}"
echo "Detected OPENH264_OS=$OPENH264_OS (uname: $(uname))"

: "${OPENH264_VERSION:=2.6.0}"
: "${FFMPEG_VERSION:=n7.1}"
: "${LAME_VERSION:=3.100}"
: "${DAV1D_VERSION:=1.4.3}"
# Pinned to the last release *before* SVT-AV1 v3.0.0's breaking API
# change (confirmed directly against both headers, not guessed): v3.0.0
# dropped the middle `p_app_data` argument from `svt_av1_enc_init_handle`
# and removed/renamed `enable_adaptive_quantization` on
# `EbSvtAv1EncConfiguration`, both of which ffmpeg n7.1's own
# libavcodec/libsvtav1.c (tagged 2024-09-30, five months before v3.0.0's
# 2025-02-20 release - it was never written against the new API) still
# expects. A newer SVT-AV1 tag fails to build against ffmpeg n7.1 with a
# real compile error ("no member named 'enable_adaptive_quantization'",
# "too many arguments to function call") - not something FFMPEG_VERSION
# or FFMPEG_CONFIGURE_EXTRA can work around; the ffmpeg version and the
# SVT-AV1 version are coupled to each other, not independently upgradable
# without also patching libsvtav1.c to match.
: "${SVT_AV1_VERSION:=v2.3.0}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$OUT_DIR"

echo "== Building openh264 v${OPENH264_VERSION} (headers/link stub only - not bundled, see ffmpeg_path.rs) =="
git clone --depth 1 --branch "v${OPENH264_VERSION}" https://github.com/cisco/openh264.git "$work/openh264"
# openh264's own build bakes a *versioned* soname/install-name into the
# library by default (e.g. libopenh264.so.8, or an absolute
# /prefix/lib/libopenh264.8.dylib on macOS) - fine for openh264's own
# releases, but this copy is never actually distributed (only linked
# against, for headers/ABI - the real runtime library is Cisco's official
# binary, downloaded separately, see the module docs in ffmpeg_path.rs),
# so what matters is that ffmpeg's own build records a *simple, fixed*
# expected filename that matches exactly what gets bundled/downloaded at
# runtime. SHAREDLIBSUFFIXMAJORVER=$(SHAREDLIBSUFFIX) collapses the
# versioned suffix away on Linux/BSD (verified directly: produces a
# library whose real ELF SONAME is exactly "libopenh264.so", confirmed
# with readelf, and a full ffmpeg build+link+run+encode against it,
# swapped for Cisco's real binary at runtime, worked end to end). MinGW
# already produces a simple "libopenh264.dll" with no override needed.
# macOS needs a different fix (its install-name is an absolute build path,
# not a simple suffixed name) - handled below via install_name_tool.
soname_suffix_override=""
case "$OPENH264_OS" in
	linux) soname_suffix_override="SHAREDLIBSUFFIXMAJORVER=so" ;;
esac
# USE_ASM=No (darwin/arm64 only): this copy of openh264 is never shipped
# or executed - only linked against for headers/ABI (see the module docs
# in ffmpeg_path.rs) - so its own codegen speed doesn't matter, only that
# it builds. Its NEON AArch64 assembly failed to compile on the macOS
# arm64 GitHub Actions runner's toolchain (multiple .o files silently
# missing, then `ar` erroring that they don't exist - a known openh264/
# Apple Silicon toolchain issue, see cisco/openh264#3353), so asm is
# disabled for just this combination. NOT applied more broadly: disabling
# it for the macOS x86_64 cross-compile target broke that build instead -
# build/platform-darwin.mk only adds `-arch x86_64` to CFLAGS/LDFLAGS
# inside its `ASM_ARCH == x86` branch (arm64 gets `-arch arm64`
# unconditionally, an asymmetry in openh264's own Makefile), and
# ASM_ARCH is only set when USE_ASM=Yes - so disabling asm there silently
# dropped the arch flag entirely, producing a host-arch (arm64) library
# that ffmpeg's `require_pkg_config` link-tested against `-arch x86_64`
# and rejected as "openh264 >= 1.3.0 not found using pkg-config" (a
# generic error covering both real absence and this kind of functional
# check failure) on the first real CI run after the fix above.
openh264_asm_override=""
if [ "$OPENH264_OS" = "darwin" ] && [ "$OPENH264_ARCH" = "arm64" ]; then
	openh264_asm_override="USE_ASM=No"
fi
# shellcheck disable=SC2086
make -C "$work/openh264" OS="$OPENH264_OS" ARCH="$OPENH264_ARCH" $openh264_asm_override $soname_suffix_override -j"$(nproc 2>/dev/null || sysctl -n hw.ncpu)"
if [ "$OPENH264_OS" = "linux" ]; then
	# The Makefile's own symlink chain assumes the majorver suffix and
	# the plain suffix differ (`.so -> .so.MAJORVER -> .so.FULLVER`) -
	# forcing them equal above leaves a self-referential `.so` symlink,
	# so it's fixed up directly here before anything links against it.
	real="$(find "$work/openh264" -maxdepth 1 -name 'libopenh264.so.*' -not -name '*.so' | head -n1)"
	ln -sf "$(basename "$real")" "$work/openh264/libopenh264.so"
fi
# shellcheck disable=SC2086
make -C "$work/openh264" install OS="$OPENH264_OS" ARCH="$OPENH264_ARCH" $openh264_asm_override $soname_suffix_override PREFIX="$work/openh264-install"
if [ "$OPENH264_OS" = "darwin" ]; then
	dylib="$(find "$work/openh264-install" -name 'libopenh264*.dylib' -not -name '*.dylib.dSYM' | head -n1)"
	install_name_tool -id "@rpath/libopenh264.dylib" "$dylib"
fi

echo "== Building LAME v${LAME_VERSION} (statically linked - MP3 patents expired 2017) =="
curl -sL "https://sourceforge.net/projects/lame/files/lame/${LAME_VERSION}/lame-${LAME_VERSION}.tar.gz/download" -o "$work/lame.tar.gz"
tar xzf "$work/lame.tar.gz" -C "$work"
(
	cd "$work/lame-${LAME_VERSION}"
	# `eval` (not a plain unquoted expansion) so a value in
	# LAME_CONFIGURE_EXTRA can itself contain a quoted, space-containing
	# argument (e.g. --cc="clang -arch x86_64") and have that quoting
	# actually respected, rather than being word-split on every space
	# regardless of quotes - which is exactly what broke the first real
	# CI run of this script for the macOS cross-compile case.
	eval ./configure --prefix="$work/lame-install" --enable-static --disable-shared --disable-frontend "$LAME_CONFIGURE_EXTRA"
	make -j"$(nproc 2>/dev/null || sysctl -n hw.ncpu)"
	make install
)

echo "== Building dav1d v${DAV1D_VERSION} (statically linked - BSD-2-Clause software AV1 decoder) =="
git clone --depth 1 --branch "$DAV1D_VERSION" https://code.videolan.org/videolan/dav1d.git "$work/dav1d-src"
(
	cd "$work/dav1d-src"
	dav1d_meson_args=(build --prefix="$work/dav1d-install" --libdir=lib --default-library=static --buildtype=release -Denable_tools=false -Denable_tests=false)
	# Cross-compiling (currently only the macOS x86_64-on-arm64 target,
	# which is the one entry that sets CC - see LAME's own build above,
	# which piggybacks on this exact same env var for the exact same
	# reason): unlike autotools, meson can't just take an overridden CC -
	# it needs an explicit cross file naming the *target* machine, since
	# (unlike LAME's build) it tries to run little test binaries as part
	# of its own compiler/feature checks, which would silently be built
	# for the wrong arch and fail to execute without one telling it not
	# to even try.
	if [ -n "${CC:-}" ]; then
		read -r -a cc_words <<<"$CC"
		cc_toml="$(printf "'%s', " "${cc_words[@]}")"
		cross_file="$work/dav1d-cross.ini"
		cat >"$cross_file" <<EOF
[binaries]
c = [${cc_toml%, }]

[host_machine]
system = 'darwin'
cpu_family = 'x86_64'
cpu = 'x86_64'
endian = 'little'
EOF
		dav1d_meson_args+=(--cross-file="$cross_file")
	fi
	meson setup "${dav1d_meson_args[@]}"
	ninja -C build
	ninja -C build install
)

echo "== Building SVT-AV1 ${SVT_AV1_VERSION} (statically linked - BSD-2-Clause-Patent AV1 encoder) =="
git clone --depth 1 --branch "$SVT_AV1_VERSION" https://gitlab.com/AOMediaCodec/SVT-AV1.git "$work/svt-av1-src"
(
	cd "$work/svt-av1-src"
	# Unset (subshell-scoped only): CC is set globally in this script's
	# environment for LAME's autotools cross-compile build above
	# (e.g. "clang -arch x86_64" for the macOS x86_64-on-arm64 target) -
	# autotools accepts that compound "compiler + flags" form, but CMake's
	# own compiler detection doesn't reliably, and CMAKE_OSX_ARCHITECTURES
	# below is already the correct, CMake-native way to cross-arch on
	# macOS - a redundant/conflicting CC on top of it is a plausible
	# culprit for the built library ending up the wrong arch without
	# CMake's own configure step visibly failing (it builds and installs
	# "successfully" either way, just possibly for the host's arch rather
	# than the requested target one).
	unset CC
	# BUILD_APPS/BUILD_TESTING off: this app only ever links the encoder
	# library itself (via ffmpeg's libsvtav1 wrapper) - the standalone
	# SvtAv1EncApp CLI and SVT-AV1's own test suite would just be extra
	# build time and dependencies (e.g. the CLI app needs its own CLI-
	# parsing deps) for output this project never uses.
	# shellcheck disable=SC2086
	# -DCMAKE_INSTALL_LIBDIR=lib: pinned explicitly (not left to
	# GNUInstallDirs' own default) for the same reason dav1d's Meson build
	# above pins --libdir=lib - some Linux distros default to lib64
	# instead, which would silently break the plain "lib/pkgconfig" path
	# ffmpeg's own PKG_CONFIG_PATH is pointed at below.
	# -DCMAKE_POLICY_VERSION_MINIMUM=3.5: SVT-AV1 v2.3.0 vendors a
	# third_party/cpuinfo submodule whose own CMakeLists.txt still
	# declares `cmake_minimum_required(VERSION <3.5)` - CMake 4.0 (already
	# what macos-latest/windows-latest's runner images ship, though not
	# yet ubuntu-22.04's, which is why this only broke 2 of the 4 targets)
	# removed support for that outright ("Compatibility with CMake < 3.5
	# has been removed from CMake"). This is CMake's own documented
	# workaround (quoted verbatim in that exact error message) for a
	# vendored third-party subproject we don't control the CMakeLists.txt
	# of - not something fixable by bumping a flag on SVT-AV1's own build.
	cmake -S . -B build -G "Unix Makefiles" \
		-DCMAKE_BUILD_TYPE=Release \
		-DCMAKE_INSTALL_PREFIX="$work/svt-av1-install" \
		-DCMAKE_INSTALL_LIBDIR=lib \
		-DCMAKE_POLICY_VERSION_MINIMUM=3.5 \
		-DBUILD_SHARED_LIBS=OFF \
		-DBUILD_APPS=OFF \
		-DBUILD_TESTING=OFF \
		$SVT_AV1_CMAKE_EXTRA
	cmake --build build --parallel "$(nproc 2>/dev/null || sysctl -n hw.ncpu)"
	cmake --install build
	# Works around a real gap in SVT-AV1 v2.3.0's own build, confirmed
	# directly against its Source/Lib/CMakeLists.txt: on platforms where
	# it can't do build-time CPUID probing (only the macOS x86_64-on-
	# arm64 cross target here - a native build probes the host's own
	# CPU directly instead), it links its vendored third_party/cpuinfo
	# for runtime CPU-feature detection via a plain CMake
	# `target_link_libraries(SvtAv1Enc PRIVATE cpuinfo_public)` - which
	# only threads through CMake's own build graph, not into the
	# separately-templated SvtAv1Enc.pc a plain pkg-config consumer
	# (ffmpeg's configure) reads, so that consumer never learns it also
	# needs -lcpuinfo (confirmed the hard way: a real CI run linked fine
	# up to "_cpuinfo_isa"/"_cpuinfo_x86_mach_init" undefined symbols).
	# Patches the installed .pc file directly instead of hardcoding a
	# guessed library name - self-adapting to whatever this build
	# actually produced, and a genuine no-op (nothing found, nothing
	# changed) on every platform that doesn't hit this gap.
	cpuinfo_lib="$(find build -name 'libcpuinfo*.a' 2>/dev/null | head -n1)"
	if [ -n "$cpuinfo_lib" ]; then
		cpuinfo_dir="$(cd "$(dirname "$cpuinfo_lib")" && pwd)"
		cpuinfo_name="$(basename "$cpuinfo_lib" .a)"
		cpuinfo_name="${cpuinfo_name#lib}"
		pc_file="$work/svt-av1-install/lib/pkgconfig/SvtAv1Enc.pc"
		awk -v extra=" -L$cpuinfo_dir -l$cpuinfo_name" \
			'/^Libs:/ { print $0 extra; next } { print }' \
			"$pc_file" >"$pc_file.tmp"
		mv "$pc_file.tmp" "$pc_file"
		echo "Patched $pc_file: appended -L$cpuinfo_dir -l$cpuinfo_name"
	fi
)

echo "== Building ffmpeg ${FFMPEG_VERSION} (LGPL only, libopenh264 + libmp3lame + libdav1d + libsvtav1) =="
git clone --depth 1 --branch "$FFMPEG_VERSION" https://github.com/FFmpeg/FFmpeg.git "$work/ffmpeg-src"
(
	cd "$work/ffmpeg-src"
	export PKG_CONFIG_PATH="$work/openh264-install/lib/pkgconfig:$work/dav1d-install/lib/pkgconfig:$work/svt-av1-install/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
	# --disable-autodetect is load-bearing, not just tidiness: without it,
	# `configure` silently links against whatever matching-named codec
	# libraries happen to already be installed on the build machine
	# (found the hard way - a build host with its own system ffmpeg/
	# codec packages produced a binary that dynamically linked against
	# *those* instead of the LGPL-only libraries actually built here,
	# including at least one nonfree codec none of our own flags
	# requested). This flag makes `configure` use only the libraries
	# explicitly `--enable-`d below, nothing auto-discovered.
	#
	# `eval` (not a plain unquoted expansion) for the same reason as
	# LAME's configure above - lets a value in FFMPEG_CONFIGURE_EXTRA
	# quote its own space-containing argument correctly (e.g.
	# --cc="clang -arch x86_64" for the macOS cross-compile case) instead
	# of every space in it being treated as a new-argument boundary
	# regardless of quoting, which broke the first real CI run of this
	# exact case ("Unknown option \"x86_64\"." - the value's own quotes
	# were being word-split away instead of respected).
	# --pkg-config-flags=--static: this ffmpeg build is fully static
	# (--enable-static --disable-shared below) - without this,
	# `configure`'s own pkg-config probing for libdav1d/libsvtav1 only
	# picks up their *dynamic*-link flags (plain `pkg-config --libs dav1d`),
	# dropping the transitive/private link deps a static consumer needs
	# (their .pc files' own `Libs.private`, e.g. libm/libpthread on Unix) -
	# the same class of "works dynamically linked, silently missing
	# symbols/flags statically linked" gap this project's openh264/LAME
	# linking already had to work around, just via pkg-config's own
	# static-query mode this time instead of an install-name/soname fix.
	# On failure, dump config.log's own tail before re-raising: configure's
	# own stdout (what's visible in a plain CI log) never includes the
	# *actual* pkg-config command/output behind a "not found" error, only
	# that generic message - config.log is the only place that's recorded,
	# and without printing it here a failure here is a dead end to
	# diagnose from the CI log alone.
	if ! eval ./configure \
		--disable-autodetect \
		--disable-gpl \
		--disable-nonfree \
		--enable-static \
		--disable-shared \
		--pkg-config-flags=--static \
		--enable-libopenh264 \
		--enable-encoder=libopenh264 \
		--enable-libmp3lame \
		--enable-libdav1d \
		--enable-libsvtav1 \
		--enable-encoder=libsvtav1 \
		--extra-cflags="-I$work/lame-install/include" \
		--extra-ldflags="-L$work/lame-install/lib" \
		--disable-doc \
		--disable-debug \
		--disable-ffplay \
		--disable-ffprobe \
		"$FFMPEG_CONFIGURE_EXTRA"; then
		echo "== ffmpeg configure failed - dumping ffbuild/config.log tail =="
		tail -n 200 ffbuild/config.log 2>/dev/null || true
		exit 1
	fi
	make -j"$(nproc 2>/dev/null || sysctl -n hw.ncpu)"
)

# MinGW's build of ffmpeg names its own output "ffmpeg.exe" already;
# every other target names it plain "ffmpeg" - check both rather than
# assuming which one this run produced.
exe_name="${FFMPEG_EXE_NAME:-ffmpeg}"
if [ -f "$work/ffmpeg-src/ffmpeg.exe" ]; then
	cp "$work/ffmpeg-src/ffmpeg.exe" "$OUT_DIR/$exe_name"
else
	cp "$work/ffmpeg-src/ffmpeg" "$OUT_DIR/$exe_name"
fi
chmod +x "$OUT_DIR/$exe_name" 2>/dev/null || true
echo "== Done: $OUT_DIR/$exe_name =="
"$OUT_DIR/$exe_name" -version | head -3 || true
