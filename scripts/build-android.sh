#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
android_dir="$repo_root/android"

usage() {
  echo "usage: $0 {debug|check|release|signed|all}"
}

case "${1:-}" in
  debug) tasks=(assembleDebug) ;;
  check) tasks=(lintDebug testDebugUnitTest) ;;
  release) tasks=(assembleRelease bundleRelease) ;;
  signed)
    required=(
      ANDROID_RELEASE_STORE_FILE
      ANDROID_RELEASE_STORE_PASSWORD
      ANDROID_RELEASE_KEY_ALIAS
      ANDROID_RELEASE_KEY_PASSWORD
    )
    for name in "${required[@]}"; do
      if [[ -z "${!name:-}" ]]; then
        echo "error: $name is required for a signed build" >&2
        exit 2
      fi
    done
    tasks=(assembleRelease bundleRelease)
    ;;
  all) tasks=(lintDebug testDebugUnitTest assembleDebug assembleRelease bundleRelease) ;;
  *) usage >&2; exit 2 ;;
esac

if [[ -x "$android_dir/gradlew" ]]; then
  gradle=("$android_dir/gradlew")
elif command -v gradle >/dev/null 2>&1; then
  gradle=(gradle)
else
  echo "error: Gradle 8.13 is required (no android/gradlew or gradle on PATH)" >&2
  exit 1
fi

cd "$android_dir"
"${gradle[@]}" --no-daemon --stacktrace "${tasks[@]}"
