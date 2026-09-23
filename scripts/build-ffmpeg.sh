#!/usr/bin/env bash
# Builds this app's bundled ffmpeg from source: LGPL-only (no x264/x265),
# with H.264 encoding via openh264 (linked at build time for the headers/
# ABI only - the actual runtime library is a separately-downloaded copy of
# Cisco's own official binary, not this build's; see ffmpeg_path.rs for
# why) and MP3 encoding via LAME (statically linked - LGPL, and MP3's
# patents expired in 2017, so no equivalent runtime-swap dance is needed).
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

set -euo pipefail

: "${OPENH264_ARCH:?}"
: "${OUT_DIR:?}"
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
# shellcheck disable=SC2086
make -C "$work/openh264" OS="$OPENH264_OS" ARCH="$OPENH264_ARCH" $soname_suffix_override -j"$(nproc 2>/dev/null || sysctl -n hw.ncpu)"
if [ "$OPENH264_OS" = "linux" ]; then
	# The Makefile's own symlink chain assumes the majorver suffix and
	# the plain suffix differ (`.so -> .so.MAJORVER -> .so.FULLVER`) -
	# forcing them equal above leaves a self-referential `.so` symlink,
	# so it's fixed up directly here before anything links against it.
	real="$(find "$work/openh264" -maxdepth 1 -name 'libopenh264.so.*' -not -name '*.so' | head -n1)"
	ln -sf "$(basename "$real")" "$work/openh264/libopenh264.so"
fi
# shellcheck disable=SC2086
make -C "$work/openh264" install OS="$OPENH264_OS" ARCH="$OPENH264_ARCH" $soname_suffix_override PREFIX="$work/openh264-install"
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

echo "== Building ffmpeg ${FFMPEG_VERSION} (LGPL only, libopenh264 + libmp3lame) =="
git clone --depth 1 --branch "$FFMPEG_VERSION" https://github.com/FFmpeg/FFmpeg.git "$work/ffmpeg-src"
(
	cd "$work/ffmpeg-src"
	export PKG_CONFIG_PATH="$work/openh264-install/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
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
	eval ./configure \
		--disable-autodetect \
		--disable-gpl \
		--disable-nonfree \
		--enable-static \
		--disable-shared \
		--enable-libopenh264 \
		--enable-encoder=libopenh264 \
		--enable-libmp3lame \
		--extra-cflags="-I$work/lame-install/include" \
		--extra-ldflags="-L$work/lame-install/lib" \
		--disable-doc \
		--disable-debug \
		--disable-ffplay \
		--disable-ffprobe \
		"$FFMPEG_CONFIGURE_EXTRA"
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
