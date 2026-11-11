#!/bin/bash
# Build the GPUI client and install it in /Applications while the running copy keeps running.
#   install.sh release   /Applications/Wheel GPUI.app (com.piercemakes.wheel.gpui, Wheel.app's daemon)
#   install.sh dev       /Applications/Wheel GPUI Dev.app (com.piercemakes.wheel.gpui.dev, Wheel Dev.app's daemon)
# tools/build.py builds and signs the bundle; tools/swap-app.sh (the step the Swift
# install-app.sh ends with) stages it, stops only the processes running from the installed bundle,
# swaps the two with renames and relaunches. wheeld is never touched (ADR-0020). Output also goes
# to ~/Library/Logs/wheel/deploy-gpui-<variant>.log, which keeps the record when the pane that
# ran this goes away with the app.
set -euo pipefail

VARIANT="${1:-}"
CLIENT="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$(cd "$CLIENT/../.." && pwd)"
case "$VARIANT" in
  release) DEST="/Applications/Wheel GPUI.app" ;;
  dev) DEST="/Applications/Wheel GPUI Dev.app" ;;
  *) echo "usage: install.sh release|dev" >&2; exit 2 ;;
esac

trap '' HUP PIPE
LOG_DIR="$HOME/Library/Logs/wheel"
LOG="$LOG_DIR/deploy-gpui-$VARIANT.log"
mkdir -p "$LOG_DIR"
exec > >(tee -a "$LOG") 2>&1
say() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*"; }
say "install gpui $VARIANT from $CLIENT"

BUILD_LOG="$LOG_DIR/deploy-gpui-$VARIANT-build.log"
say "building $(basename "$DEST"); the build's output is in $BUILD_LOG"
# build.py's last line is the bundle it made; everything else it and its tools print is in the log.
if ! python3 "$CLIENT/tools/build.py" "$VARIANT" >"$BUILD_LOG" 2>&1; then
  tail -5 "$BUILD_LOG"
  say "build failed (see $BUILD_LOG); nothing was installed"
  exit 1
fi
grep -E '^(warning: no Apple Development|embedded wheeld)' "$BUILD_LOG" || true
APP="$(tail -1 "$BUILD_LOG")"
[ -d "$APP" ] || { say "build.py did not name the bundle it made (last line: $APP)"; exit 1; }
say "built $APP"
bash "$ROOT/tools/swap-app.sh" "$APP" "$DEST"
