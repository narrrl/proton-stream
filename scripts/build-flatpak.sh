#!/usr/bin/env bash
# Build the Flatpak from this checkout, offline, as Flathub would.
#
#   scripts/build-flatpak.sh            regenerate cargo-sources.json, build, bundle
#   scripts/build-flatpak.sh --install  ...and install it for this user
#   scripts/build-flatpak.sh --sources  only regenerate cargo-sources.json
#
# Needs flatpak, flatpak-builder and python3 with aiohttp and tomlkit (for
# flatpak-cargo-generator). The GNOME SDK and the rust-stable extension are
# installed from Flathub on first run. The bundle lands in dist/.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd -- "$script_dir/.." && pwd)"
APP_ID=io.narl.proton-stream
MANIFEST="$ROOT/packaging/flatpak/$APP_ID.yml"
WORK="$ROOT/.build/flatpak"
# Pinned so a regenerated cargo-sources.json is reproducible.
TOOLS_REV=9d86d336a39b9af38b86f114b6ce5820fe260f91
GENERATOR="$WORK/flatpak-cargo-generator-$TOOLS_REV.py"

die() { echo "error: $*" >&2; exit 1; }

install=false
sources_only=false
for arg in "$@"; do
  case "$arg" in
    --install) install=true ;;
    --sources) sources_only=true ;;
    *) die "unknown argument: $arg (see the header of $0)" ;;
  esac
done

# Flathub builds the registry crates; a path patch cannot resolve in its sandbox.
grep -q '^\[patch\.crates-io\]' "$ROOT/Cargo.toml" &&
  die "Cargo.toml carries [patch.crates-io]; the Flatpak builds against crates.io"

mkdir -p "$WORK" "$ROOT/dist"
if [[ ! -f "$GENERATOR" ]]; then
  curl -fsSL -o "$GENERATOR.tmp" \
    "https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/$TOOLS_REV/cargo/flatpak-cargo-generator.py"
  mv "$GENERATOR.tmp" "$GENERATOR"
fi
python3 -I "$GENERATOR" "$ROOT/Cargo.lock" -o "$ROOT/packaging/flatpak/cargo-sources.json"
echo "regenerated packaging/flatpak/cargo-sources.json"
$sources_only && exit 0

version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)"
bundle="$ROOT/dist/proton-stream-$version-x86_64.flatpak"

flatpak-builder --user --install-deps-from=flathub --force-clean --ccache \
  --state-dir="$WORK/state" --repo="$WORK/repo" \
  $($install && echo --install) \
  "$WORK/build" "$MANIFEST"
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
  "$WORK/repo" "$bundle" "$APP_ID"
echo "built ${bundle#$ROOT/}"
