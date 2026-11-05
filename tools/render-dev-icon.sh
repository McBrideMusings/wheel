#!/bin/bash
# Render the Wheel Dev app icon: the committed icon with a red DEV band across its lower-right
# corner, written to apple/Mac/Assets.xcassets/AppIconDev.appiconset. `make icon-render` runs it
# after rendering the plain icon; the dev build selects the set with ASSETCATALOG_COMPILER_APPICON_NAME.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/apple/Mac/Assets.xcassets/AppIcon.appiconset"
OUT="$ROOT/apple/Mac/Assets.xcassets/AppIconDev.appiconset"
mkdir -p "$OUT"
cp "$SRC/Contents.json" "$OUT/Contents.json"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cat >"$work/banner.svg" <<'SVG'
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <g transform="translate(72 72) rotate(-35)">
    <rect x="-100" y="-11" width="200" height="22" fill="rgb(220,40,40)" fill-opacity="0.94"/>
    <text x="0" y="5" font-family="Helvetica" font-weight="bold" font-size="14" fill="white" text-anchor="middle">DEV</text>
  </g>
</svg>
SVG
for png in "$SRC"/*.png; do
  size="$(basename "$png" .png)"
  resvg -w "$size" -h "$size" "$work/banner.svg" "$work/banner-$size.png"
  magick "$png" "$work/banner-$size.png" -composite -alpha off "$OUT/$size.png"
done
