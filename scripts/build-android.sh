#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
android_dir="$repo_root/android"

usage() {
  echo "usage: $0 {debug|check|release|signed|all}"
  echo
  echo "  check  ktlint, Android lint, host unit tests and screenshot comparison —"
  echo "         no device needed. Re-record changed screens with"
  echo "         'gradlew recordRoborazziDebug' and commit the images."
  echo "         On-device coverage is scripts/android-acceptance.sh."
}

case "${1:-}" in
  debug) tasks=(assembleDebug) ;;
  # verifyRoborazziDebug *is* testDebugUnitTest, with the screenshots compared
  # against src/test/screenshots instead of skipped.
  check) tasks=(ktlintCheck lintDebug verifyRoborazziDebug) ;;
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
  all) tasks=(ktlintCheck lintDebug verifyRoborazziDebug assembleDebug assembleRelease bundleRelease) ;;
  *) usage >&2; exit 2 ;;
esac

# The Android toolchain needs JDK 17. A newer JVM does not fail with a version
# message — the Kotlin compiler throws `IllegalArgumentException: 26.0.2` out of
# an IntelliJ version parser, which reads as a broken build rather than a wrong
# JDK. Find 17 before that can happen.
java_major() {
  local java_bin="$1"
  [[ -x "$java_bin" ]] || return 1
  "$java_bin" -version 2>&1 | sed -n '1s/.*version "\([0-9]*\).*/\1/p'
}

if [[ "$(java_major "${JAVA_HOME:-}/bin/java" || true)" != "17" ]]; then
  found=""
  for candidate in \
    /usr/lib/jvm/java-17-openjdk \
    /usr/lib/jvm/java-17-openjdk-amd64 \
    /usr/lib/jvm/temurin-17-jdk \
    /Library/Java/JavaVirtualMachines/temurin-17.jdk/Contents/Home; do
    if [[ "$(java_major "$candidate/bin/java" || true)" == "17" ]]; then
      found="$candidate"
      break
    fi
  done
  if [[ -z "$found" ]] && [[ "$(java_major "$(command -v java || echo /nonexistent)" || true)" == "17" ]]; then
    found="$(dirname "$(dirname "$(readlink -f "$(command -v java)")")")"
  fi
  if [[ -z "$found" ]]; then
    echo "error: JDK 17 is required (see docs/ANDROID.md); set JAVA_HOME to it" >&2
    exit 1
  fi
  export JAVA_HOME="$found"
fi

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
