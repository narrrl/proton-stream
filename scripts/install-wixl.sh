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
  $SUDO apt-get install -y -qq --no-install-recommends \
    build-essential ninja-build valac bison pkg-config gettext git python3-pip \
    libgcab-dev libgirepository1.0-dev libgsf-1-dev libxml2-dev uuid-dev >/dev/null
fi

# The install step below runs as root, and `meson install` re-executes meson
# there — so a pip --user meson in ~/.local is useless to it: the launcher is on
# disk but `mesonbuild` is not on root's sys.path, and ninja stops with
# ModuleNotFoundError after a complete build. Install it system-wide, under the
# same $SUDO that will run the install, and use that binary explicitly rather
# than whatever PATH resolves — a stale ~/.local/bin/meson would shadow it.
MESON="$(command -v meson || true)"
meson_usable() {
  [[ -n "$MESON" ]] || return 1
  [[ "$(printf '1.4\n%s\n' "$("$MESON" --version 2>/dev/null)" | sort -V | head -1)" == "1.4" ]] || return 1
  # Whoever performs the install has to be able to import mesonbuild.
  $SUDO "$MESON" --version >/dev/null 2>&1
}

if ! meson_usable; then
  log "installing meson >= 1.4 from pip"
  $SUDO pip3 install --break-system-packages --root-user-action=ignore --quiet 'meson>=1.4' \
    || $SUDO pip3 install --quiet 'meson>=1.4'
  hash -r 2>/dev/null || true
  MESON="$([[ -x /usr/local/bin/meson ]] && echo /usr/local/bin/meson || command -v meson)"
  meson_usable || die "meson >= 1.4 is still not usable as the installing user"
fi

src="$(mktemp -d)"
build_log="$(mktemp -t wixl-build-XXXXXX.log)"
trap 'rm -rf "$src"' EXIT

log "cloning msitools $MSITOOLS_TAG"
git clone -q --depth 1 --branch "$MSITOOLS_TAG" --recursive "$REPO" "$src/msitools" 2>/dev/null

# valac generates C that warns about everything; a clean build is some 600 lines
# of noise that buries whatever went wrong. Keep the log and print its tail only
# when a step fails. The log sits outside $src so the trap does not take it.
run() {
  if ! "$@" >>"$build_log" 2>&1; then
    tail -40 "$build_log" >&2
    die "$1 failed; full log was $build_log"
  fi
}

log "building"
run "$MESON" setup "$src/msitools/build" "$src/msitools" --prefix "$PREFIX" --buildtype=release
run ninja -C "$src/msitools/build"

log "installing into $PREFIX"
if [[ -w "$PREFIX" ]]; then
  run "$MESON" install -C "$src/msitools/build" --no-rebuild
else
  run $SUDO "$MESON" install -C "$src/msitools/build" --no-rebuild
fi
[[ $EUID -eq 0 || -n "$SUDO" ]] && $SUDO ldconfig 2>/dev/null || true

# A distro wixl left in place would still win on PATH; say which one answers.
hash -r 2>/dev/null || true
have wixl || die "$PREFIX/bin is not on PATH"
log "wixl $(wixl --version) at $(command -v wixl)"
