#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
source_dir="${PSTR_MPV_SOURCE_DIR:-$repo_root/.build/mpv-android}"
revision=20a3fa526fac6d3fe267aee0d4c349893fee65a3
destination="$repo_root/android/app/build/generated/mpv"

if [[ ! -d "$source_dir/.git" ]]; then
  git clone https://github.com/mpv-android/mpv-android.git "$source_dir"
fi
git -C "$source_dir" fetch --depth=1 origin "$revision"
git -C "$source_dir" checkout --detach "$revision"
git -C "$source_dir" submodule sync --recursive
git -C "$source_dir" submodule update --init --recursive

if [[ -n "${ANDROID_NDK_HOME:-}" ]]; then
  ndk_dir="$ANDROID_NDK_HOME"
elif [[ -n "${ANDROID_HOME:-}" ]]; then
  ndk_dir="$ANDROID_HOME/ndk/29.0.14206865"
else
  echo "error: set ANDROID_NDK_HOME or ANDROID_HOME to an Android NDK r29 installation" >&2
  exit 1
fi
if [[ ! -f "$ndk_dir/source.properties" ]] ||
   ! grep -q '^Pkg.Revision = 29\.0\.14206865$' "$ndk_dir/source.properties"; then
  echo "error: libmpv requires Android NDK 29.0.14206865: $ndk_dir" >&2
  exit 1
fi
ndk_dir="$(cd -- "$ndk_dir" && pwd -P)"
mpv_sdk_dir="$source_dir/buildscripts/sdk"
mpv_ndk_link="$mpv_sdk_dir/android-ndk-r29"
mkdir -p "$mpv_sdk_dir"
if [[ -e "$mpv_ndk_link" && ! -L "$mpv_ndk_link" ]]; then
  echo "error: refusing to replace non-symlink NDK path: $mpv_ndk_link" >&2
  exit 1
fi
ln -sfn "$ndk_dir" "$mpv_ndk_link"

ensure_lua_source() {
  local lua_dir="$source_dir/buildscripts/deps/lua"
  if [[ -f "$lua_dir/Makefile" ]]; then
    return
  fi

  local lua_url="https://launchpad.net/ubuntu/+archive/primary/+sourcefiles/lua5.2/5.2.4-3build2/lua5.2_5.2.4.orig.tar.gz"
  local lua_sha256="86fb7e23cbbddfcd92684e5f8017ff41c9112251d1656dbece415a97fad171c0"
  local lua_archive
  local lua_staging
  lua_archive="$(mktemp)"
  lua_staging="$(mktemp -d)"
  if ! curl --fail --location --retry 3 --output "$lua_archive" "$lua_url"; then
    rm -f -- "$lua_archive"
    rm -rf -- "$lua_staging"
    echo "error: failed to download the pinned Lua 5.2.4 source" >&2
    exit 1
  fi
  if ! printf '%s  %s\n' "$lua_sha256" "$lua_archive" | sha256sum --check --status; then
    rm -f -- "$lua_archive"
    rm -rf -- "$lua_staging"
    echo "error: Lua 5.2.4 source checksum mismatch" >&2
    exit 1
  fi
  mkdir -p "$source_dir/buildscripts/deps"
  tar -xzf "$lua_archive" -C "$lua_staging" --strip-components=1
  rm -f -- "$lua_archive"
  if [[ -e "$lua_dir" || -L "$lua_dir" ]]; then
    # This path is fixed beneath the generated dependency directory, and the
    # valid-source early return above prevents replacing a completed checkout.
    rm -rf -- "$lua_dir"
  fi
  mv "$lua_staging" "$lua_dir"
}

# This pinned mpv-android revision stores its dependency recipes in-tree and
# materializes their exact source revisions under buildscripts/deps. Some of
# those dependencies contain submodules of their own. The downloader is
# idempotent and must run before buildall can resolve the `mpv` target.
ensure_lua_source
(cd "$source_dir/buildscripts" && ./include/download-deps.sh)
ensure_lua_source

for arch in arm64 x86_64; do
  (cd "$source_dir/buildscripts" && ./buildall.sh --arch "$arch" mpv)
done

llvm_readelf="$(find "$ndk_dir/toolchains/llvm/prebuilt" -name llvm-readelf -print -quit)"
if [[ -z "$llvm_readelf" ]]; then
  echo "error: llvm-readelf is missing from Android NDK: $ndk_dir" >&2
  exit 1
fi

# These libraries are supplied by Android itself. Every other DT_NEEDED entry
# must be packaged in the APK for each ABI. Keep this list explicit: silently
# treating an unknown library as a platform library produces installable APKs
# that fail as soon as the dynamic linker loads pstr_mpv/libmpv.
is_android_system_library() {
  case "$1" in
    libc.so | libdl.so | liblog.so | libm.so | libz.so | libandroid.so | \
      libmediandk.so | libnativewindow.so | libOpenSLES.so | libaaudio.so | \
      libEGL.so | libGLESv1_CM.so | libGLESv2.so | libGLESv3.so | \
      libjnigraphics.so | libvulkan.so)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

stage_shared_dependency_closure() {
  local arch="$1"
  local abi="$2"
  local ndk_triple="$3"
  local prefix_lib="$source_dir/buildscripts/prefix/$arch/lib"
  local ndk_lib="$ndk_dir/toolchains/llvm/prebuilt/$(basename "$(dirname "$(dirname "$llvm_readelf")")")/sysroot/usr/lib/$ndk_triple"
  local output_dir="$destination/jniLibs/$abi"
  local current needed source soname
  local -a pending=(libmpv.so)
  local -A staged=()

  # Remove stale libraries from an earlier build before constructing the exact
  # closure for this pinned revision.
  mkdir -p "$output_dir"
  find "$output_dir" -maxdepth 1 -type f -name '*.so' -delete

  while ((${#pending[@]})); do
    current="${pending[0]}"
    pending=("${pending[@]:1}")
    [[ -n "${staged[$current]:-}" ]] && continue

    if [[ -f "$prefix_lib/$current" ]]; then
      source="$prefix_lib/$current"
    elif [[ "$current" == libc++_shared.so && -f "$ndk_lib/$current" ]]; then
      source="$ndk_lib/$current"
    else
      echo "error: unresolved non-system dependency for $abi: $current" >&2
      exit 1
    fi

    soname="$($llvm_readelf -d "$source" | sed -n 's/.*SONAME.*\[\(.*\)\]/\1/p')"
    if [[ "$soname" != "$current" ]]; then
      echo "error: $source has SONAME '$soname', expected '$current'" >&2
      exit 1
    fi
    cp -- "$source" "$output_dir/$current"
    staged[$current]=1

    while IFS= read -r needed; do
      if ! is_android_system_library "$needed" && [[ -z "${staged[$needed]:-}" ]]; then
        pending+=("$needed")
      fi
    done < <("$llvm_readelf" -d "$source" | sed -n 's/.*Shared library: \[\(.*\)\]/\1/p')
  done

  # Re-read only the packaged copies, so verification also catches a bad or
  # versioned staging filename rather than trusting the source tree.
  for current in "${!staged[@]}"; do
    while IFS= read -r needed; do
      if ! is_android_system_library "$needed" && [[ ! -f "$output_dir/$needed" ]]; then
        echo "error: $abi/$current needs unstaged library $needed" >&2
        exit 1
      fi
    done < <("$llvm_readelf" -d "$output_dir/$current" | sed -n 's/.*Shared library: \[\(.*\)\]/\1/p')
  done

  printf 'staged %s shared libraries for %s:\n' "${#staged[@]}" "$abi"
  printf '  %s\n' "${!staged[@]}" | LC_ALL=C sort
}

mkdir -p "$destination/include/mpv"
cp "$source_dir/buildscripts/prefix/arm64/include/mpv/"*.h "$destination/include/mpv/"
stage_shared_dependency_closure arm64 arm64-v8a aarch64-linux-android
stage_shared_dependency_closure x86_64 x86_64 x86_64-linux-android

# `git archive` omits dependency working trees. Archive the pinned checkout and
# downloaded sources together, excluding only generated build/install output
# and Git administration data, so distributed binaries have corresponding
# source for mpv and every statically linked dependency.
tar \
  --exclude='.git' \
  --exclude='buildscripts/prefix' \
  --exclude='buildscripts/sdk' \
  --exclude='buildscripts/deps/*/_build_*' \
  -C "$source_dir" \
  -czf "$destination/mpv-android-source-$revision.tar.gz" \
  .
printf '%s\n' "$revision" > "$destination/REVISION"
