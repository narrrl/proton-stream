#!/usr/bin/env bash
#
# Windows release build + packaging, hosted on Linux. No Windows, no Wine.
#
#   scripts/build-windows.sh --fetch-mpv          # everything, fetching libmpv
#   scripts/build-windows.sh --mpv-dev ~/mpv-dev  # everything, using a local copy
#   scripts/build-windows.sh --skip-build msi     # repackage what is already staged
#
# Artifacts land in dist/:
#   dist/windows/                                 the portable payload
#   dist/proton-stream-<version>-x86_64-windows.zip
#   dist/proton-stream-<version>-x64.msi
#
# This is the cross-compiled counterpart to scripts/build.ps1, which needs MSVC
# and WiX and therefore a real Windows box. Two things differ, and both are
# deliberate:
#
#   * The target is x86_64-pc-windows-gnu, not -msvc. mingw-w64 is a real cross
#     compiler; MSVC is not one you can install here. The ABI difference is
#     invisible to this application — everything it links is either Rust or
#     compiled from source by a build script (aws-lc-sys, libsqlite3-sys), and
#     the one prebuilt dependency, libmpv, is loaded through a DLL import
#     library that mingw ships an equivalent of.
#
#   * The .msi is built by wixl (msitools), not WiX, from
#     packaging/windows/proton-stream.wixl.wxs. WiX cannot run off Windows at
#     all; that file's header explains exactly where it breaks. The cost is the
#     installer's dialog UI. See the same header.
#
# Prerequisites (Arch names; every distro has them):
#   mingw-w64-gcc  msitools  zip                 pacman -S mingw-w64-gcc msitools zip
#   Debian/Ubuntu split wixl out of msitools:    apt install mingw-w64 wixl zip
#   rustup target add x86_64-pc-windows-gnu
#   osslsigncode                                 only for --sign

set -euo pipefail

readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly DIST="$ROOT/dist"
readonly OUTDIR="$DIST/windows"
readonly PKG="$ROOT/packaging/windows"

readonly PKGNAME="proton-stream"
readonly GUI_BIN="proton-stream.exe"
readonly CLI_BIN="pstr.exe"

# The mpv-dev archive: libmpv-2.dll plus the import library to link against.
# mpv's *player* build ships neither.
#
# Fetched from shinchiro's releases rather than the SourceForge mirror the
# archive is better known by: SourceForge's `/download` URLs answer anything
# that does not look like a browser with an HTML interstitial, which lands as a
# file called mpv-dev.7z that is not an archive. These are the same builds — the
# SourceForge project mirrors them.
readonly MPV_RELEASE="https://api.github.com/repos/shinchiro/mpv-winbuild-cmake/releases/latest"

VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)"
readonly VERSION

# ---------------------------------------------------------------- options ---

TARGET="x86_64-pc-windows-gnu"
MPV_DEV="${MPV_DEV_DIR:-}"
FETCH_MPV=0
SKIP_BUILD=0
SIGN_CERT=""
SIGN_KEY=""
TIMESTAMP_URL="http://timestamp.digicert.com"
TARGETS=()

usage() {
  cat <<EOF
usage: scripts/build-windows.sh [options] [artifact ...]

artifacts
  mpv         resolve libmpv's development files into dist/mpv-dev and stop
  binaries    cargo build --release --target ${TARGET}, then stage dist/windows
  zip         dist/${PKGNAME}-${VERSION}-x86_64-windows.zip
  msi         dist/${PKGNAME}-${VERSION}-x64.msi                (needs wixl)
  all         all of the above (the default)

options
  --mpv-dev <dir>   directory holding libmpv-2.dll and libmpv.dll.a (or mpv.def)
  --fetch-mpv       download the latest mpv-dev archive into dist/mpv-dev
  --target <triple> default ${TARGET}
  --skip-build      reuse target/<triple>/release from a previous run
  --sign <cert.pem> Authenticode-sign the binaries and the .msi (needs --sign-key)
  --sign-key <key>  the private key for --sign
  --timestamp <url> RFC 3161 timestamp server, default ${TIMESTAMP_URL}
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mpv-dev)   MPV_DEV="$2"; shift 2 ;;
    --fetch-mpv) FETCH_MPV=1; shift ;;
    --target)    TARGET="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --sign)      SIGN_CERT="$2"; shift 2 ;;
    --sign-key)  SIGN_KEY="$2"; shift 2 ;;
    --timestamp) TIMESTAMP_URL="$2"; shift 2 ;;
    -h|--help)   usage; exit 0 ;;
    -*)          echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)           TARGETS+=("$1"); shift ;;
  esac
done

[[ ${#TARGETS[@]} -eq 0 ]] && TARGETS=(all)
for t in "${TARGETS[@]}"; do
  case "$t" in
    mpv|binaries|zip|msi|all) ;;
    *) echo "unknown artifact: $t" >&2; usage >&2; exit 2 ;;
  esac
done
if [[ " ${TARGETS[*]} " == *" all "* ]]; then
  TARGETS=(binaries zip msi)
fi

log()  { printf '\033[35m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[33mwarning:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# ------------------------------------------------------------------ libmpv ---

# Newest x86_64 mpv-dev build. The "v3" variants are x86-64-v3 microarchitecture
# builds and would refuse to load on older CPUs, so they are filtered out.
fetch_mpv() {
  local dest="$DIST/mpv-dev"
  if [[ -f "$dest/libmpv-2.dll" ]]; then
    log "reusing $dest"
    MPV_DEV="$dest"
    return
  fi

  have curl || die "--fetch-mpv needs curl"
  have 7z   || die "--fetch-mpv needs 7z (p7zip)"

  # The asset names are mpv-dev-<arch>-<date>-git-<sha>.7z; the date is what
  # keeps this off the i686, aarch64 and v3 builds in the same release.
  local url
  url="$(curl -sL --max-time 60 ${GITHUB_TOKEN:+-H "Authorization: Bearer $GITHUB_TOKEN"} "$MPV_RELEASE" \
         | grep -o '"browser_download_url": *"[^"]*mpv-dev-x86_64-20[0-9]*-git-[^"]*\.7z"' \
         | sed 's|.*: *"||; s|"$||' | head -1)"
  [[ -n "$url" ]] || die "no mpv-dev build in the latest shinchiro/mpv-winbuild-cmake release"

  log "downloading $(basename "$url")"
  mkdir -p "$dest"
  curl -L --max-time 600 -o "$dest.7z" "$url"
  # A download that fetched an error page instead of the archive fails inside 7z
  # with something unrelated to what went wrong; say it here.
  [[ "$(head -c2 "$dest.7z")" == "7z" ]] || die "$url did not return a 7z archive"
  7z x -y -o"$dest" "$dest.7z" >/dev/null
  rm -f "$dest.7z"
  MPV_DEV="$dest"
}

# Resolves MPV_DEV to a directory containing both libmpv-2.dll and an import
# library the linker will find as -lmpv.
resolve_mpv() {
  [[ $FETCH_MPV -eq 1 ]] && fetch_mpv

  # A previous --fetch-mpv left its unpacked copy in dist/mpv-dev. Adopt it
  # rather than making every later run repeat the flag; it is the same
  # directory fetch_mpv would have reused.
  if [[ -z "$MPV_DEV" && -f "$DIST/mpv-dev/libmpv-2.dll" ]]; then
    log "reusing $DIST/mpv-dev"
    MPV_DEV="$DIST/mpv-dev"
  fi

  [[ -n "$MPV_DEV" ]] || die "$(cat <<EOF
no libmpv development files. Either pass --fetch-mpv, or download an
mpv-dev-x86_64-*.7z from
  https://github.com/shinchiro/mpv-winbuild-cmake/releases/latest
unpack it, and pass --mpv-dev <dir> or set MPV_DEV_DIR.
EOF
)"
  [[ -d "$MPV_DEV" ]] || die "--mpv-dev path does not exist: $MPV_DEV"

  local dll
  dll="$(find "$MPV_DEV" -name libmpv-2.dll -print -quit)"
  [[ -n "$dll" ]] || die "libmpv-2.dll not found under $MPV_DEV"
  MPV_DIR="$(dirname "$dll")"
  MPV_DLL="$dll"

  # The archive normally ships libmpv.dll.a, which is exactly what
  # `-lmpv` resolves to under mingw. Older ones ship only the .def.
  if [[ ! -f "$MPV_DIR/libmpv.dll.a" ]]; then
    [[ -f "$MPV_DIR/mpv.def" ]] \
      || die "neither libmpv.dll.a nor mpv.def in $MPV_DIR — not an mpv-dev build"
    have x86_64-w64-mingw32-dlltool || die "need x86_64-w64-mingw32-dlltool to build the import library"
    log 'generating libmpv.dll.a from mpv.def'
    # -D matters: without it the import library would reference mpv.dll, and the
    # file that actually ships is libmpv-2.dll.
    x86_64-w64-mingw32-dlltool -d "$MPV_DIR/mpv.def" -D libmpv-2.dll \
                               -l "$MPV_DIR/libmpv.dll.a"
  fi
}

# ------------------------------------------------------------------- build ---

build() {
  if [[ $SKIP_BUILD -eq 1 ]]; then log 'skipping cargo build'; return; fi

  have x86_64-w64-mingw32-gcc || die 'no mingw-w64 cross compiler (pacman -S mingw-w64-gcc)'
  rustup target list --installed | grep -qx "$TARGET" \
    || die "rust target $TARGET not installed — rustup target add $TARGET"

  # The dev-only path patch; say so plainly rather than deep in a resolve error.
  if grep -q '^\[patch\.crates-io\]' "$ROOT/Cargo.toml" && [[ ! -d "$ROOT/../proton-sdk-rs" ]]; then
    die 'Cargo.toml still carries [patch.crates-io] -> ../proton-sdk-rs, but that checkout is missing.'
  fi

  log "cargo build --release --target $TARGET"
  # libmpv2-sys emits `cargo:rustc-link-lib=mpv` and nothing else, so the search
  # path has to come from outside. Appended rather than assigned: a RUSTFLAGS
  # already in the environment is the caller's, and dropping it silently changes
  # what gets built.
  RUSTFLAGS="${RUSTFLAGS:-} -L native=$MPV_DIR" \
    cargo build --release --locked --target "$TARGET" --bin proton-stream --bin pstr \
    --manifest-path "$ROOT/Cargo.toml"
}

stage() {
  log 'staging into dist/windows'
  local rel="$ROOT/target/$TARGET/release"
  for f in "$GUI_BIN" "$CLI_BIN"; do
    [[ -f "$rel/$f" ]] || die "$f missing from $rel — drop --skip-build"
  done

  rm -rf "$OUTDIR"
  mkdir -p "$OUTDIR"
  install -m755 "$rel/$GUI_BIN" "$rel/$CLI_BIN" "$MPV_DLL" "$OUTDIR/"
  install -m644 "$ROOT/README.md" "$OUTDIR/"

  if [[ -n "$SIGN_CERT" ]]; then
    sign "$OUTDIR/$GUI_BIN"
    sign "$OUTDIR/$CLI_BIN"
  fi
}

# signtool.exe has no Linux build; osslsigncode produces the same Authenticode
# signature from a PEM certificate and key.
sign() {
  local path="$1"
  have osslsigncode || die 'signing needs osslsigncode'
  [[ -n "$SIGN_KEY" ]] || die '--sign needs --sign-key'
  log "signing $(basename "$path")"
  osslsigncode sign -certs "$SIGN_CERT" -key "$SIGN_KEY" \
    -h sha256 -ts "$TIMESTAMP_URL" \
    -n 'Proton Stream' -i 'https://github.com/narrrl/proton-stream' \
    -in "$path" -out "$path.signed" >/dev/null
  mv "$path.signed" "$path"
}

# ---------------------------------------------------------------- portable ---

make_zip() {
  local out="$DIST/${PKGNAME}-${VERSION}-x86_64-windows.zip"
  log "zip -> $(basename "$out")"
  have zip || die 'no zip on PATH'
  rm -f "$out"
  (cd "$OUTDIR" && zip -qr9 "$out" .)
}

# --------------------------------------------------------------------- msi ---

make_msi() {
  # Debian and Ubuntu ship wixl in its own binary package; their `msitools`
  # package has the msi* tools and not this one. Arch puts both in msitools.
  have wixl || die 'the .msi needs wixl (pacman -S msitools / apt install wixl)'

  local out="$DIST/${PKGNAME}-${VERSION}-x64.msi"
  log "msi -> $(basename "$out")"
  rm -f "$out"

  # wixl reads the payload out of dist/windows, so a --sign run signs the
  # binaries before they are packed rather than only the .msi around them.
  wixl --arch x64 \
    -D "Version=$VERSION" \
    -D "BinDir=$OUTDIR" \
    -D "MpvDll=$OUTDIR/libmpv-2.dll" \
    -D "IconFile=$PKG/proton-stream.ico" \
    -D "ReadMe=$OUTDIR/README.md" \
    -o "$out" "$PKG/proton-stream.wixl.wxs"

  [[ -n "$SIGN_CERT" ]] && sign "$out"
  return 0
}

# -------------------------------------------------------------------- main ---

mkdir -p "$DIST"
# Only the compile and the staging copy need libmpv; repackaging out of
# dist/windows does not, so `--skip-build msi` works with no mpv-dev at hand.
if [[ " ${TARGETS[*]} " == *" binaries "* || " ${TARGETS[*]} " == *" mpv "* ]]; then
  resolve_mpv
fi

for t in "${TARGETS[@]}"; do
  case "$t" in
    mpv)      log "libmpv at $MPV_DIR" ;;   # resolve_mpv above did the work
    binaries) build; stage ;;
    zip)      [[ -d "$OUTDIR" ]] || die 'nothing staged — run the binaries artifact first'; make_zip ;;
    msi)      [[ -d "$OUTDIR" ]] || die 'nothing staged — run the binaries artifact first'; make_msi ;;
  esac
done

log 'done — dist/'
find "$DIST" -maxdepth 1 -type f -printf '    %10s bytes  %f\n' | sort -k3
