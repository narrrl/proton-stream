#!/usr/bin/env bash
# Build the iOS client as an unsigned .ipa for SideStore / AltStore.
#
# SideStore re-signs whatever it installs with the viewer's own Apple ID, so
# nothing here touches a certificate or a provisioning profile. Needs macOS with
# Xcode, XcodeGen and the aarch64-apple-ios Rust target.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
ios_dir="$repo_root/ios"
generated="$ios_dir/build/generated"
target=aarch64-apple-ios

usage() {
  echo "usage: $0 {rust|bindings|ipa|all}"
  echo
  echo "  rust      static libpstr_android.a for $target"
  echo "  bindings  Swift bindings into ios/build/generated"
  echo "  ipa       xcodegen + xcodebuild, then dist/ProtonStream-<version>.ipa"
  echo "  all       all three, in that order"
}

die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }
log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

[[ "$(uname -s)" == Darwin ]] || die "the iOS build needs macOS and Xcode"

# Same guard as scripts/build.sh: without the sibling checkout the resolve fails
# a hundred lines deep in cargo output.
if grep -q '^\[patch\.crates-io\]' "$repo_root/Cargo.toml" && [[ ! -d "$repo_root/../proton-sdk-rs" ]]; then
  die "Cargo.toml still carries [patch.crates-io] -> ../proton-sdk-rs, but that checkout is missing."
fi

version="$(awk '/^\[workspace.package\]/{found=1; next} found && /^\[/{exit} found && /^version = /{gsub(/[" ]/, "", $3); print $3; exit}' "$repo_root/Cargo.toml")"
[[ -n "$version" ]] || die "no [workspace.package] version in Cargo.toml"

build_rust() {
  log "cargo rustc -p pstr-android --target $target (staticlib)"
  # The crate stays `cdylib, rlib` for Android; the archive iOS links is asked
  # for here instead of being built on every other platform too.
  ( cd "$repo_root" && IPHONEOS_DEPLOYMENT_TARGET=17.0 cargo rustc -p pstr-android --lib \
      --release --locked --target "$target" --crate-type staticlib )
}

build_bindings() {
  log "uniffi-bindgen --language swift"
  # Library mode reads the interface out of a built library, and the host
  # dylib is the one the bindgen binary can open.
  ( cd "$repo_root" && cargo build -p pstr-android --lib --locked )
  rm -rf "$generated"
  ( cd "$repo_root" && cargo run -p pstr-android --features bindgen --bin uniffi-bindgen --locked -- \
      generate --library target/debug/libpstr_android.dylib --language swift \
      --no-format --out-dir "$generated" )
  [[ -f "$generated/PstrBridge.swift" ]] || die "bindgen wrote no PstrBridge.swift"
}

build_ipa() {
  command -v xcodegen >/dev/null || die "xcodegen is required (brew install xcodegen)"
  [[ -f "$repo_root/target/$target/release/libpstr_android.a" ]] || die "run '$0 rust' first"
  [[ -f "$generated/PstrBridge.swift" ]] || die "run '$0 bindings' first"

  log "xcodegen ($version)"
  ( cd "$ios_dir" && PSTR_VERSION="$version" PSTR_BUILD="${PSTR_BUILD:-1}" xcodegen generate --quiet )

  log "xcodebuild (unsigned)"
  xcodebuild -project "$ios_dir/ProtonStream.xcodeproj" -scheme ProtonStream \
    -configuration Release -sdk iphoneos -destination 'generic/platform=iOS' \
    -derivedDataPath "$ios_dir/build/DerivedData" \
    CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY= \
    build

  local app="$ios_dir/build/DerivedData/Build/Products/Release-iphoneos/ProtonStream.app"
  [[ -d "$app" ]] || die "xcodebuild produced no ProtonStream.app"
  local stage="$ios_dir/build/ipa"
  rm -rf "$stage"
  mkdir -p "$stage/Payload" "$repo_root/dist"
  cp -R "$app" "$stage/Payload/"
  local ipa="$repo_root/dist/ProtonStream-$version.ipa"
  rm -f "$ipa"
  ( cd "$stage" && zip -qry "$ipa" Payload )
  log "wrote $ipa"
}

case "${1:-}" in
  rust) build_rust ;;
  bindings) build_bindings ;;
  ipa) build_ipa ;;
  all) build_rust; build_bindings; build_ipa ;;
  *) usage >&2; exit 2 ;;
esac
