#!/bin/bash
# Install a built app bundle over the one in place while the running copy keeps running:
#   swap-app.sh <built app> <destination app>
# The bundle is staged beside the destination with ditto, the running app is stopped only for the
# two renames that swap the bundles, and the new one is relaunched with a Finder-like environment.
# Only processes whose executable lies inside the destination bundle stop, so wheeld (which runs
# from a copy in Application Support) keeps running (ADR-0020). The caller owns the log: this
# script writes to stdout, and install-app.sh and clients/gpui/tools/install.sh tee it.
set -euo pipefail

APP="${1:-}"
DEST="${2:-}"
[ -d "$APP" ] && [ -n "$DEST" ] || { echo "usage: swap-app.sh <built app> <destination app>" >&2; exit 2; }

# The swap stops the app this may be running inside; a hangup or a closed pipe from that pane
# must not stop the install half-way.
trap '' HUP PIPE
say() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*"; }

LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
WORK="$(dirname "$DEST")/.$(basename "$DEST").installing.noindex"
STAGED="$WORK/new/$(basename "$DEST")"
PREVIOUS="$WORK/previous/$(basename "$DEST")"
cleanup_work() {
  rm -rf "$WORK"
  "$LSREGISTER" -u "$STAGED" 2>/dev/null || true
  "$LSREGISTER" -u "$PREVIOUS" 2>/dev/null || true
}
# One exit handler for every run: it removes the staging folder.
trap cleanup_work EXIT

# Staged with ditto (a signed bundle depends on its exact symlinks) in a hidden .noindex folder
# beside the destination, which Spotlight does not list, while the old app still runs.
rm -rf "$WORK"
mkdir -p "$(dirname "$STAGED")" "$(dirname "$PREVIOUS")"
ditto "$APP" "$STAGED"
say "staged $STAGED"

# Only processes whose executable lies inside the installed bundle stop.
running_from() {
  ps -Ao pid=,comm= | awk -v prefix="$DEST/" '{ pid = $1; sub(/^[ \t]*[0-9]+[ \t]+/, ""); if (index($0, prefix) == 1) print pid }'
}
wait_gone() {
  local tenths="$1"
  while [ "$tenths" -gt 0 ]; do
    [ -z "$(running_from)" ] && return 0
    sleep 0.1
    tenths=$((tenths - 1))
  done
  [ -z "$(running_from)" ]
}
PIDS="$(running_from | tr '\n' ' ')"
if [ -n "$PIDS" ]; then
  say "stopping $(basename "$DEST") (pids: $PIDS)"
  # shellcheck disable=SC2086 # one word per pid
  kill -TERM $PIDS 2>/dev/null || true
  if ! wait_gone 50; then
    say "$(basename "$DEST") ignored SIGTERM, sending SIGKILL"
    # shellcheck disable=SC2046 # one word per pid
    kill -KILL $(running_from) 2>/dev/null || true
    wait_gone 20 || true
  fi
fi

if [ -e "$DEST" ]; then
  mv "$DEST" "$PREVIOUS"
fi
if ! mv "$STAGED" "$DEST"; then
  [ -e "$PREVIOUS" ] && mv "$PREVIOUS" "$DEST"
  say "could not swap in the new bundle; the previous one is back"
  exit 1
fi
touch "$DEST"
"$LSREGISTER" -f "$DEST" || true
say "installed $DEST"

# Relaunched with only what a Finder launch carries, so the app does not start as a client of
# the pane that ran this (WHEEL_URL, WHEEL_PANE and the rest).
LAUNCH_ENV=()
while IFS='=' read -r key value; do
  case "$key" in
    HOME | USER | LOGNAME | SHELL | PATH | TMPDIR | LANG | __CF_USER_TEXT_ENCODING | LC_*) LAUNCH_ENV+=("$key=$value") ;;
  esac
done < <(env)
env -i "${LAUNCH_ENV[@]}" open "$DEST"
say "launched $DEST"
