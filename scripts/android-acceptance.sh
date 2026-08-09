#!/usr/bin/env bash
# On-device acceptance for the Android client. Drives a real device or emulator
# through the matrix docs/ANDROID.md requires before Android work is submitted.
set -Eeuo pipefail

usage() {
    cat >&2 <<'EOF'
usage: scripts/android-acceptance.sh [OPTION ...]

Runs against one attached device or emulator. The app is installed from the
debug APK unless --no-install is given; nothing here needs a Proton account
except the cases marked "live", which skip cleanly without one.

Options:
  --serial SERIAL     target this device (default: the only attached one)
  --apk PATH          install this APK instead of the debug output
  --no-install        use whatever build is already installed
  --list              print every case and what it needs, then exit
  --fail-fast         stop at the first failure instead of continuing
  --timeout SECONDS   per-case limit (default 120)
  --report-json PATH  machine-readable results
  --report-junit PATH JUnit XML for CI
  --keep-going-on-crash
                      report a crash as a failure and continue rather than
                      aborting the run

Environment:
  PSTR_ACCEPTANCE_ONLY=name[,name]   run only these cases (substring match)
  PSTR_ACCEPTANCE_SHARE_URL          public link for the live cases
  PSTR_ACCEPTANCE_SHARE_PASSWORD     custom password, if the link needs one

Live cases are destructive to the app's own state: they clear app data between
phases. They never write to the share.
EOF
    exit 2
}

case "${1:-}" in
    -h|--help) usage ;;
esac

command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 2; }
command -v adb >/dev/null || { echo "adb is required (Android platform-tools)" >&2; exit 2; }

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
exec python3 -u "$script_dir/android-acceptance.py" "$@"
