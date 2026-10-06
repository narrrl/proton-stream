#!/usr/bin/env bash
# Render ios/ProtonStream/Resources/AppIcon.svg into the asset catalog's one
# 1024 px icon. iOS wants it opaque, so the alpha channel is dropped.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
resources="$script_dir/../ios/ProtonStream/Resources"
out="$resources/Assets.xcassets/AppIcon.appiconset/AppIcon-1024.png"

command -v rsvg-convert >/dev/null || { echo "rsvg-convert is required (librsvg)" >&2; exit 1; }
command -v magick >/dev/null || { echo "magick is required (ImageMagick)" >&2; exit 1; }
rsvg-convert -w 1024 -h 1024 "$resources/AppIcon.svg" | magick - -alpha off -strip "$out"
echo "wrote $out"
