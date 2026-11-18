#!/usr/bin/env bash
# Compatibility entry point: all platform icons share images/logo.svg.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
exec python3 "$script_dir/render-branding.py" "$@"
