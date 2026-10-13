#!/usr/bin/env bash
# Render the Android TV launcher banner, 320 by 180 dp, from the iOS icon: its
# canvas widened to 16:9 with the face gradient run across the whole width,
# and the play mark a little smaller, since a banner is shown as is rather
# than cut down by a mask. Twice the size, in drawable-nodpi: one file for
# every screen, which the launcher scales to its slot, sharp on a 4K one.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
icon="$script_dir/../ios/ProtonStream/Resources/AppIcon.svg"
out="$script_dir/../android/app/src/main/res/drawable-nodpi/tv_banner.png"

command -v rsvg-convert >/dev/null || { echo "rsvg-convert is required (librsvg)" >&2; exit 1; }
command -v magick >/dev/null || { echo "magick is required (ImageMagick)" >&2; exit 1; }
mkdir -p "$(dirname "$out")"
sed -e 's/width="1024" height="1024" viewBox="0 0 108 108"/width="320" height="180" viewBox="-42 0 192 108"/' \
    -e 's/<rect width="108" height="108"/<rect x="-42" width="192" height="108"/' \
    -e 's/x1="0" y1="0" x2="108" y2="108"/x1="-42" y1="0" x2="150" y2="108"/' \
    -e 's/scale(1.5)/scale(1.15)/' \
    "$icon" | rsvg-convert -w 640 -h 360 | magick - -alpha off -strip "$out"
echo "wrote $out"
