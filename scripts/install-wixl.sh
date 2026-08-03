#!/usr/bin/env bash
#
# Builds msitools' wixl from source, for hosts whose distribution ships one too
# old to package proton-stream.
#
#   scripts/install-wixl.sh                 # build + install if wixl < 0.105
#   scripts/install-wixl.sh --force         # build even if a new enough one is here
#   scripts/install-wixl.sh --prefix ~/.local
#   scripts/install-wixl.sh --no-deps       # do not touch apt
#
# Why this exists
# ===============
# packaging/windows/proton-stream.wixl.wxs puts the PATH entry for pstr.exe in
# an <Environment> element. wixl only learned that element in msitools 0.105;
# older ones abort on it, and not gracefully:
#
#   ** (wixl): ERROR: wix.vala:232: unhandled child Component node Environment
#   Trace/breakpoint trap (core dumped)
#
# Arch has 0.106, so a hand-built release is fine. Ubuntu 24.04 — the release
# workflow's runner — has 0.103, and 0.106 first appears in 25.10, so there is
# nothing to apt-install and nothing to backport: the questing .deb wants
# libxml2-16, which noble does not have. Hence a source build.
#
# Installs to /usr/local by default, with sudo when not already root.

set -euo pipefail

readonly MSITOOLS_TAG="v0.106"
readonly MIN_VERSION="0.105"
readonly REPO="https://gitlab.gnome.org/GNOME/msitools.git"

PREFIX="/usr/local"
FORCE=0
DEPS=1

usage() {
  cat <<EOF
usage: scripts/install-wixl.sh [--prefix DIR] [--force] [--no-deps]

Builds msitools ${MSITOOLS_TAG} and installs wixl into PREFIX (default ${PREFIX}).
Does nothing if wixl ${MIN_VERSION} or newer is already on PATH.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)   PREFIX="$2"; shift 2 ;;
    --force)    FORCE=1; shift ;;
    --no-deps)  DEPS=0; shift ;;
    -h|--help)  usage; exit 0 ;;
    *)          echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

log()  { printf '\033[35m==>\033[0m %s\n' "$*"; }
die()  { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# Root in a container, sudo on a runner or a workstation.
SUDO=""
[[ $EUID -eq 0 ]] || SUDO="sudo"

if [[ $FORCE -eq 0 ]] && have wixl; then
  current="$(wixl --version 2>/dev/null | head -1)"
  if [[ -n "$current" && "$(printf '%s\n%s\n' "$MIN_VERSION" "$current" | sort -V | head -1)" == "$MIN_VERSION" ]]; then
    log "wixl $current is new enough (>= $MIN_VERSION), nothing to do"
    exit 0
  fi
  log "wixl ${current:-unknown} predates <Environment> support, building $MSITOOLS_TAG"
fi

# Ubuntu 24.04's meson is 1.3.2 and msitools wants >= 1.4, so meson comes from
# pip rather than apt. gobject-introspection is not optional in libmsi's
# meson.build, and the bats test harness is a submodule — hence --recursive
# below rather than a release tarball, which does not carry submodules.
if [[ $DEPS -eq 1 ]] && have apt-get; then
  log "installing build dependencies"
  $SUDO apt-get update -qq
  $SUDO apt-get install -y --no-install-recommends \
    build-essential ninja-build valac bison pkg-config gettext git python3-pip \
    libgcab-dev libgirepository1.0-dev libgsf-1-dev libxml2-dev uuid-dev
fi

if ! have meson || [[ "$(printf '1.4\n%s\n' "$(meson --version)" | sort -V | head -1)" != "1.4" ]]; then
  log "installing meson >= 1.4 from pip"
  pip3 install --break-system-packages --quiet 'meson>=1.4' \
    || pip3 install --quiet 'meson>=1.4'
fi

src="$(mktemp -d)"
trap 'rm -rf "$src"' EXIT

log "cloning msitools $MSITOOLS_TAG"
git clone -q --depth 1 --branch "$MSITOOLS_TAG" --recursive "$REPO" "$src/msitools"

log "building"
meson setup "$src/msitools/build" "$src/msitools" --prefix "$PREFIX" --buildtype=release
ninja -C "$src/msitools/build"

log "installing into $PREFIX"
if [[ -w "$PREFIX" ]]; then
  ninja -C "$src/msitools/build" install
else
  $SUDO ninja -C "$src/msitools/build" install
fi
[[ $EUID -eq 0 || -n "$SUDO" ]] && $SUDO ldconfig 2>/dev/null || true

# A distro wixl left in place would still win on PATH; say which one answers.
hash -r 2>/dev/null || true
have wixl || die "$PREFIX/bin is not on PATH"
log "wixl $(wixl --version) at $(command -v wixl)"
