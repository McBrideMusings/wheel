#!/bin/bash
# Render apple/Mac/ServerIcon/ServerIcon.svg to ServerIcon.icns, the icon of Wheel Server.app.
# Change the colours in the SVG, run this, and commit both files.
set -euo pipefail
DIR="$(cd "$(dirname "$0")/../apple/Mac/ServerIcon" && pwd)"
SET="$(mktemp -d)/ServerIcon.iconset"
mkdir -p "$SET"
for size in 16 32 128 256 512; do
  rsvg-convert -w $size -h $size "$DIR/ServerIcon.svg" -o "$SET/icon_${size}x${size}.png"
  rsvg-convert -w $((size * 2)) -h $((size * 2)) "$DIR/ServerIcon.svg" -o "$SET/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$SET" -o "$DIR/ServerIcon.icns"
echo "wrote $DIR/ServerIcon.icns"
