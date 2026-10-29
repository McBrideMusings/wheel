#!/usr/bin/env bash
# Unregister Launch Services entries for Wheel.app copies outside /Applications. Build output,
# DerivedData and scratch copies stay registered after their builds are gone, and Raycast and
# Spotlight list them beside the installed app. Removes only Wheel.app and Wheel Dev.app paths
# outside /Applications, prints each one, then prints what is still registered.
# Usage: lsregister-prune.sh [--dry-run]
set -euo pipefail

LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
DRY_RUN=0
[ "${1:-}" = "--dry-run" ] && DRY_RUN=1

# One bundle path per registration; the dump prints `path: <path> (0x<id>)`.
paths="$("$LSREGISTER" -dump | sed -E -n 's/^ *path: +(.*) \(0x[0-9a-f]+\)$/\1/p' \
  | grep -E '/Wheel( Dev)?\.app$' | sort -u || true)"

removed=0
kept=0
while IFS= read -r p; do
  [ -z "$p" ] && continue
  case "$p" in
    /Applications/*) kept=$((kept + 1)); echo "keep    $p"; continue ;;
  esac
  if [ "$DRY_RUN" = 1 ]; then
    echo "would remove  $p"
  else
    "$LSREGISTER" -u "$p"
    echo "removed $p"
  fi
  removed=$((removed + 1))
done <<< "$paths"

echo "$removed outside /Applications, $kept in /Applications"

# What the dump still lists outside /Applications: empty after a real run.
left="$("$LSREGISTER" -dump | sed -E -n 's/^ *path: +(.*) \(0x[0-9a-f]+\)$/\1/p' \
  | grep -E '/Wheel( Dev)?\.app$' | grep -v '^/Applications/' | sort -u || true)"
if [ -n "$left" ]; then
  echo "still registered outside /Applications:"
  echo "$left"
  [ "$DRY_RUN" = 1 ] || exit 1
fi
