#!/usr/bin/env bash
#
# Release build + packaging driver for Linux and macOS.
#
# One cargo build feeds every artifact: the binaries are compiled once, staged
# into a filesystem image under dist/stage, and each packager copies out of that
# image. So a .deb and a .rpm from the same run are the same bytes, and neither
# needs a Rust toolchain or libmpv headers on the packaging host.
#
#   scripts/build.sh                  # everything this host can produce
#   scripts/build.sh deb rpm          # just those
#   scripts/build.sh --skip-build deb # repackage without recompiling
#   scripts/build.sh --target aarch64-unknown-linux-gnu tarball
#
# Windows is scripts/build.ps1 — the .exe and the .msi need MSVC and WiX, and
# neither is reachable from here. Arch is packaging/PKGBUILD via `makepkg -fi`,
# which compiles in place by Arch convention rather than consuming dist/stage;
# `scripts/build.sh arch` runs it for you.
#
# Artifacts land in dist/.

set -euo pipefail

readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly DIST="$ROOT/dist"
readonly STAGE="$DIST/stage"

readonly APP_ID="io.narl.proton-stream"
readonly PKGNAME="proton-stream"
readonly GUI_BIN="proton-stream"
readonly CLI_BIN="pstr"

VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)"
readonly VERSION

# ---------------------------------------------------------------- options ---

TARGET=""          # cargo --target; empty means host
SKIP_BUILD=0
BUNDLE_LIBS=1      # macOS: copy libmpv & friends into the .app
TARGETS=()

usage() {
  cat <<EOF
usage: scripts/build.sh [options] [artifact ...]

artifacts
  binaries    cargo build --release, then stage a filesystem image
  tarball     dist/${PKGNAME}-${VERSION}-<triple>.tar.gz          (linux, macos)
  deb         dist/${PKGNAME}_${VERSION}-1_<arch>.deb             (linux)
  rpm         dist/${PKGNAME}-${VERSION}-1.<arch>.rpm             (linux, needs rpmbuild)
  arch        packaging/PKGBUILD via makepkg                      (linux, needs makepkg)
  app         dist/Proton Stream.app                              (macos)
  dmg         dist/${PKGNAME}-${VERSION}-<arch>.dmg               (macos)
  all         every artifact this host can produce (the default)

options
  --target <triple>   cross-compile; also names the tarball
  --skip-build        reuse whatever is already in dist/stage
  --no-bundle-libs    macOS: link against the host's libmpv instead of copying it
  -h, --help          this
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)         TARGET="$2"; shift 2 ;;
    --target=*)       TARGET="${1#*=}"; shift ;;
    --skip-build)     SKIP_BUILD=1; shift ;;
    --no-bundle-libs) BUNDLE_LIBS=0; shift ;;
    -h|--help)        usage; exit 0 ;;
    -*)               echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)                TARGETS+=("$1"); shift ;;
  esac
done

case "$(uname -s)" in
  Linux)  HOST_OS=linux ;;
  Darwin) HOST_OS=macos ;;
  *)      echo "error: $(uname -s) is not handled here — Windows is scripts/build.ps1" >&2; exit 1 ;;
esac
readonly HOST_OS

[[ ${#TARGETS[@]} -gt 0 ]] || TARGETS=(all)

# Where cargo puts the binaries, which --target moves.
if [[ -n "$TARGET" ]]; then
  RELDIR="$ROOT/target/$TARGET/release"
else
  RELDIR="$ROOT/target/release"
fi
readonly RELDIR

log()  { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# ------------------------------------------------------------------ build ---

build() {
  [[ $SKIP_BUILD -eq 0 ]] || { log "skipping cargo build"; return; }

  # The dev-only path patch. Without the sibling checkout the resolve fails a
  # hundred lines deep in cargo output; say so up front instead.
  if grep -q '^\[patch\.crates-io\]' "$ROOT/Cargo.toml" && [[ ! -d "$ROOT/../proton-sdk-rs" ]]; then
    die "Cargo.toml still carries [patch.crates-io] -> ../proton-sdk-rs, but that checkout is missing.
       Clone it beside this repo, or drop the patch block once proton-sdk is published."
  fi

  log "cargo build --release${TARGET:+ --target $TARGET}"
  ( cd "$ROOT" && cargo build --release --locked \
      ${TARGET:+--target "$TARGET"} \
      --bin "$GUI_BIN" --bin "$CLI_BIN" )
}

# `install -D` is a GNU extension and macOS does not have it, so the staging
# below goes through mkdir + cp + chmod instead.
put() {
  local mode="$1" src="$2" dst="$3"
  mkdir -p "$(dirname "$dst")"
  cp "$src" "$dst"
  chmod "$mode" "$dst"
}

# Filesystem image every Linux packager copies out of. Rooted so that
# stage/usr/bin/... is exactly where the file lands on the target system.
stage() {
  log "staging into dist/stage"
  rm -rf "$STAGE"
  put 755 "$RELDIR/$GUI_BIN" "$STAGE/usr/bin/$GUI_BIN"
  put 755 "$RELDIR/$CLI_BIN" "$STAGE/usr/bin/$CLI_BIN"
  put 644 "$ROOT/packaging/$APP_ID.desktop" \
    "$STAGE/usr/share/applications/$APP_ID.desktop"
  put 644 "$ROOT/packaging/$APP_ID.svg" \
    "$STAGE/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"
  put 644 "$ROOT/README.md" "$STAGE/usr/share/doc/$PKGNAME/README.md"

  # Both packagers list these as directories, so they have to exist even when
  # the checkout has no LICENSE yet.
  mkdir -p "$STAGE/usr/share/licenses/$PKGNAME"
  if [[ -f "$ROOT/LICENSE" ]]; then
    put 644 "$ROOT/LICENSE" "$STAGE/usr/share/licenses/$PKGNAME/LICENSE"
  else
    warn "no LICENSE in the repo root; Cargo.toml declares MIT — packages ship without the text"
  fi
}

need_stage() {
  [[ -x "$STAGE/usr/bin/$GUI_BIN" ]] || die "dist/stage is empty — drop --skip-build"
}

# ---------------------------------------------------------------- helpers ---

# Debian and RPM each spell the machine differently from the Rust triple.
deb_arch() {
  case "${TARGET:-$(uname -m)}" in
    x86_64*|amd64) echo amd64 ;;
    aarch64*|arm64) echo arm64 ;;
    *) die "no Debian architecture mapped for ${TARGET:-$(uname -m)}" ;;
  esac
}

rpm_arch() {
  case "${TARGET:-$(uname -m)}" in
    x86_64*|amd64) echo x86_64 ;;
    aarch64*|arm64) echo aarch64 ;;
    *) die "no RPM architecture mapped for ${TARGET:-$(uname -m)}" ;;
  esac
}

triple() {
  if [[ -n "$TARGET" ]]; then echo "$TARGET"
  else rustc -vV | awk '/^host:/ {print $2}'
  fi
}

# ---------------------------------------------------------------- tarball ---

do_tarball() {
  need_stage
  local name="$PKGNAME-$VERSION-$(triple)"
  local out="$DIST/$name.tar.gz"
  log "tarball -> ${out#$ROOT/}"

  local tmp; tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  mkdir -p "$tmp/$name"
  cp "$STAGE/usr/bin/$GUI_BIN" "$STAGE/usr/bin/$CLI_BIN" "$tmp/$name/"
  cp "$ROOT/README.md" "$tmp/$name/"
  cp "$ROOT/packaging/$APP_ID.svg" "$tmp/$name/"
  if [[ -f "$ROOT/LICENSE" ]]; then cp "$ROOT/LICENSE" "$tmp/$name/"; fi
  tar -C "$tmp" -czf "$out" "$name"
}

# -------------------------------------------------------------------- deb ---

do_deb() {
  # GNU tar's --owner/--group and md5sum below; not worth emulating for a
  # platform that cannot install the result anyway.
  [[ "$HOST_OS" == linux ]] || die "the .deb can only be built on Linux"
  need_stage
  local arch; arch="$(deb_arch)"
  local dir="$DIST/deb/${PKGNAME}_${VERSION}-1_${arch}"
  local out="$DIST/${PKGNAME}_${VERSION}-1_${arch}.deb"
  log "deb -> ${out#$ROOT/}"

  rm -rf "$dir"
  mkdir -p "$dir"
  cp -a "$STAGE/." "$dir/"

  # Debian keeps the copyright under doc/, not licenses/.
  rm -rf "$dir/usr/share/licenses"
  if [[ -f "$ROOT/LICENSE" ]]; then
    put 644 "$ROOT/LICENSE" "$dir/usr/share/doc/$PKGNAME/copyright"
  fi

  local size; size="$(du -sk "$dir" | cut -f1)"

  # libmpv2 is trixie and later; libmpv1 is bookworm. Neither pulls the mpv
  # binary in, which is right — nothing here shells out to mpv(1).
  # rusqlite is `bundled` and TLS is rustls, so no libsqlite3 and no libssl.
  mkdir -p "$dir/DEBIAN"
  cat > "$dir/DEBIAN/control" <<EOF
Package: $PKGNAME
Version: $VERSION-1
Section: video
Priority: optional
Architecture: $arch
Maintainer: Nils Pukropp <contact@narl.io>
Installed-Size: $size
Depends: libc6, libmpv2 | libmpv1, libsecret-1-0, libgl1, libxkbcommon0, libx11-6, libwayland-client0
Recommends: gnome-keyring
Homepage: https://github.com/narrrl/proton-stream
Description: Netflix-style desktop client for Proton Drive public links
 Paste a Proton Drive share URL and its password, and get a browsable,
 streamable library: a poster wall, a page per title with seasons and
 episodes, resume-where-you-left-off, and an embedded libmpv player.
 .
 No Proton account, no server and no download step - a file's content blocks
 each decrypt on their own, so seeking costs the blocks the seek lands on
 rather than a re-stream.
EOF

  # md5sums over everything but the control archive itself.
  ( cd "$dir" && find . -path ./DEBIAN -prune -o -type f -print0 \
      | xargs -0 md5sum | sed 's|\./||' > DEBIAN/md5sums )

  if have dpkg-deb; then
    dpkg-deb --root-owner-group --build "$dir" "$out" >/dev/null
  else
    # No dpkg on Arch without the AUR, so assemble the ar archive by hand. A
    # .deb is three members in this order: debian-binary, control, data.
    have ar || die "need either dpkg-deb or binutils' ar to build a .deb"
    local tmp; tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' RETURN
    echo "2.0" > "$tmp/debian-binary"
    tar -C "$dir/DEBIAN" --owner=0 --group=0 -czf "$tmp/control.tar.gz" .
    tar -C "$dir" --owner=0 --group=0 --exclude=./DEBIAN -cJf "$tmp/data.tar.xz" .
    rm -f "$out"
    ( cd "$tmp" && ar rcD "$out" debian-binary control.tar.gz data.tar.xz )
  fi
}

# -------------------------------------------------------------------- rpm ---

do_rpm() {
  [[ "$HOST_OS" == linux ]] || die "the .rpm can only be built on Linux"
  need_stage
  have rpmbuild || die "rpmbuild not found (Arch: pacman -S rpm-tools; Fedora: dnf install rpm-build)"
  local arch; arch="$(rpm_arch)"
  log "rpm -> dist/${PKGNAME}-${VERSION}-1.${arch}.rpm"

  local top="$DIST/rpmbuild"
  rm -rf "$top"
  mkdir -p "$top"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}

  # rpmbuild narrates every shell line it runs; keep the log and only show it
  # when something actually went wrong.
  local logf="$DIST/rpmbuild.log"
  if ! rpmbuild -bb "$ROOT/packaging/$PKGNAME.spec" \
      --define "_topdir $top" \
      --define "version $VERSION" \
      --define "stagedir $STAGE" \
      --define "dist %{nil}" \
      --target "$arch" >"$logf" 2>&1; then
    cat "$logf" >&2
    die "rpmbuild failed (log: ${logf#$ROOT/})"
  fi

  find "$top/RPMS" -name '*.rpm' -exec cp {} "$DIST/" \;
}

# ------------------------------------------------------------------- arch ---

do_arch() {
  have makepkg || die "makepkg not found — this artifact is Arch-only"
  # makepkg compiles in place from the checkout (Arch convention, and what the
  # sibling repo does), so it ignores dist/stage entirely.
  log "makepkg (packaging/PKGBUILD)"
  ( cd "$ROOT/packaging" && makepkg -f --noconfirm )
  find "$ROOT/packaging" -maxdepth 1 -name "$PKGNAME-*.pkg.tar.zst" -newermt '-5 minutes' \
    -exec cp {} "$DIST/" \;
}

# ------------------------------------------------------------------ macOS ---

# Copy a dylib's non-system dependency closure into the bundle and rewrite every
# reference to it. Homebrew's libmpv pulls in ~40 of these; without this the
# .app only runs on a machine that already did `brew install mpv`.
bundle_dylibs() {
  local app="$1" bin="$2"
  local fw="$app/Contents/Frameworks"
  mkdir -p "$fw"

  # An index cursor and a newline-delimited seen-set rather than array slicing
  # and an associative array: /bin/bash on macOS is still 3.2, which has
  # neither.
  local queue=("$bin")
  local seen=$'\n'
  local i=0

  while [[ $i -lt ${#queue[@]} ]]; do
    local item="${queue[$i]}"
    i=$((i + 1))
    local dep
    while read -r dep; do
      # System libraries stay put; they exist on every macOS.
      case "$dep" in
        /usr/lib/*|/System/*|@*) continue ;;
      esac
      local base; base="$(basename "$dep")"
      if [[ "$seen" != *$'\n'"$base"$'\n'* ]]; then
        seen="$seen$base"$'\n'
        if [[ ! -f "$dep" ]]; then
          warn "dependency not found on disk: $dep"
          continue
        fi
        cp -f "$dep" "$fw/$base"
        chmod u+w "$fw/$base"
        install_name_tool -id "@rpath/$base" "$fw/$base" 2>/dev/null || true
        queue+=("$fw/$base")
      fi
      install_name_tool -change "$dep" "@rpath/$base" "$item" 2>/dev/null || true
    done < <(otool -L "$item" | tail -n +2 | awk '{print $1}')
  done

  install_name_tool -add_rpath "@executable_path/../Frameworks" "$bin" 2>/dev/null || true
  # Ad-hoc re-sign: every install_name_tool edit invalidates the signature, and
  # an unsigned-but-modified binary is killed on arm64.
  codesign --force --deep --sign - "$app" 2>/dev/null \
    || warn "codesign failed; the .app may not launch on Apple silicon"
}

make_icns() {
  local out="$1"
  local svg="$ROOT/packaging/$APP_ID.svg"
  have iconutil || { warn "iconutil missing; .app ships without an icon"; return 1; }
  local rasterize=""
  have rsvg-convert && rasterize=rsvg-convert
  [[ -z "$rasterize" ]] && have magick && rasterize=magick
  [[ -z "$rasterize" ]] && { warn "no rsvg-convert or magick; .app ships without an icon"; return 1; }

  local iconset; iconset="$(mktemp -d)/icon.iconset"
  mkdir -p "$iconset"
  local s
  for s in 16 32 64 128 256 512 1024; do
    if [[ "$rasterize" == rsvg-convert ]]; then
      rsvg-convert -w "$s" -h "$s" "$svg" -o "$iconset/icon_${s}x${s}.png"
    else
      magick -background none "$svg" -resize "${s}x${s}" "$iconset/icon_${s}x${s}.png"
    fi
  done
  # Retina names iconutil expects.
  for s in 16 32 128 256 512; do
    cp "$iconset/icon_$((s * 2))x$((s * 2)).png" "$iconset/icon_${s}x${s}@2x.png"
  done
  iconutil -c icns "$iconset" -o "$out"
}

do_app() {
  [[ "$HOST_OS" == macos ]] || die "the .app bundle can only be built on macOS"
  need_stage
  local app="$DIST/Proton Stream.app"
  log "app bundle -> dist/Proton Stream.app"

  rm -rf "$app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

  cp "$STAGE/usr/bin/$GUI_BIN" "$app/Contents/MacOS/$GUI_BIN"
  # The CLI rides along inside the bundle rather than in /usr/local/bin: a .dmg
  # drag-install has no business writing outside /Applications.
  cp "$STAGE/usr/bin/$CLI_BIN" "$app/Contents/MacOS/$CLI_BIN"

  sed -e "s/@VERSION@/$VERSION/g" -e "s/@BIN@/$GUI_BIN/g" -e "s/@APP_ID@/$APP_ID/g" \
    "$ROOT/packaging/macos/Info.plist.in" > "$app/Contents/Info.plist"

  make_icns "$app/Contents/Resources/$PKGNAME.icns" || true

  if [[ $BUNDLE_LIBS -eq 1 ]]; then
    log "bundling dylibs into the .app"
    bundle_dylibs "$app" "$app/Contents/MacOS/$GUI_BIN"
    bundle_dylibs "$app" "$app/Contents/MacOS/$CLI_BIN"
  else
    warn "--no-bundle-libs: the .app needs \`brew install mpv\` on the target machine"
  fi
}

do_dmg() {
  [[ "$HOST_OS" == macos ]] || die "the .dmg can only be built on macOS"
  local app="$DIST/Proton Stream.app"
  [[ -d "$app" ]] || do_app
  local out="$DIST/$PKGNAME-$VERSION-$(uname -m).dmg"
  log "dmg -> ${out#$ROOT/}"

  local tmp; tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  cp -R "$app" "$tmp/"
  ln -s /Applications "$tmp/Applications"
  rm -f "$out"
  hdiutil create -volname "Proton Stream" -srcfolder "$tmp" \
    -ov -format UDZO "$out" >/dev/null
}

# ------------------------------------------------------------------- main ---

mkdir -p "$DIST"

expand_all() {
  case "$HOST_OS" in
    linux) echo "binaries tarball deb rpm" ;;
    macos) echo "binaries tarball app dmg" ;;
  esac
}

# `all` expands in place; `binaries` is implied by everything else, so it runs
# once at the front rather than per artifact.
want=()
for t in "${TARGETS[@]}"; do
  if [[ "$t" == all ]]; then
    for e in $(expand_all); do want+=("$e"); done
  else
    want+=("$t")
  fi
done

# `arch` is the one artifact that never reads dist/stage — makepkg compiles in
# place. Asking for nothing but that skips the cargo build here.
needs_stage=0
for t in "${want[@]}"; do
  [[ "$t" == arch ]] || needs_stage=1
done
if [[ $needs_stage -eq 1 ]]; then
  build
  stage
fi

for t in "${want[@]}"; do
  case "$t" in
    binaries) ;;   # already done above
    tarball)  do_tarball ;;
    deb)      do_deb ;;
    rpm)      do_rpm ;;
    arch)     do_arch ;;
    app)      do_app ;;
    dmg)      do_dmg ;;
    *)        die "unknown artifact: $t" ;;
  esac
done

log "done — dist/"
find "$DIST" -maxdepth 1 \( -type f -o -name '*.app' \) -exec du -sh {} + \
  | sed "s|$DIST/|    |"
