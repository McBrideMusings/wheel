#!/bin/bash
# Build the Mac app and install it in /Applications while the running copy keeps running.
#   install-app.sh release   Release build, installed as /Applications/Wheel.app
#   install-app.sh dev       Debug build with the .dev bundle id and the AppIconDev icon,
#                            installed as /Applications/Wheel Dev.app
# The running app is stopped only for the two renames that swap the bundles, then relaunched.
# wheeld is left running: its process is `wheeld`, not a path inside the bundle, so the stop
# never reaches it, and the app offers a server restart itself when the bundled one differs
# (ADR-0020). Output also goes to ~/Library/Logs/wheel/deploy-<variant>.log, which keeps the
# record when the pane that ran this goes away with the app.
set -euo pipefail

VARIANT="${1:-}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
case "$VARIANT" in
  release) CONFIG=Release DEST=/Applications/Wheel.app SETTINGS=() ;;
  dev) CONFIG=Debug DEST="/Applications/Wheel Dev.app" SETTINGS=(DEV_BUNDLE_SUFFIX=.dev ASSETCATALOG_COMPILER_APPICON_NAME=AppIconDev) ;;
  *) echo "usage: install-app.sh release|dev" >&2; exit 2 ;;
esac

# The swap stops the app this may be running inside; a hangup or a closed pipe from that pane
# must not stop the install half-way.
trap '' HUP PIPE
LOG_DIR="$HOME/Library/Logs/wheel"
LOG="$LOG_DIR/deploy-$VARIANT.log"
mkdir -p "$LOG_DIR"
exec > >(tee -a "$LOG") 2>&1
say() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*"; }
say "install-app $VARIANT from $ROOT"

# Sign with the Apple Development certificate by its hash, so macOS keeps the permissions it
# granted the app. With no such certificate the app is signed ad hoc.
IDENTITY="$("$ROOT/tools/signing-identity")"
if [ "$IDENTITY" = - ]; then
  say "warning: no Apple Development certificate in the keychain; signing ad hoc"
  IDENTITY=-
fi
SETTINGS+=(CODE_SIGN_IDENTITY="$IDENTITY" CODE_SIGN_STYLE=Manual PROVISIONING_PROFILE_SPECIFIER=)
[ -n "${IOS_DEVELOPMENT_TEAM:-}" ] && SETTINGS+=(DEVELOPMENT_TEAM="$IOS_DEVELOPMENT_TEAM")

# Each variant has its own derived data, so a deploy never replaces the Debug app `make
# build-client` leaves for the checks.
# `.noindex` keeps Spotlight from importing the build products.
DERIVED="$ROOT/DerivedData-deploy-$VARIANT.noindex"
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

( cd "$ROOT" && xcodegen generate --spec apple/project.yml --project apple )
BUILD_LOG="$LOG_DIR/deploy-$VARIANT-xcodebuild.log"
say "building Wheel ($CONFIG); xcodebuild's output is in $BUILD_LOG"
# The generated workspace records the package versions once resolved; reading them back skips a
# network round trip per build. A project that was never resolved resolves on its first build.
RESOLVE=()
[ -f "$ROOT/apple/Wheel.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved" ] \
  && RESOLVE=(-disableAutomaticPackageResolution)
status=0
xcodebuild -project "$ROOT/apple/Wheel.xcodeproj" -scheme Wheel -configuration "$CONFIG" \
  -destination platform=macOS -derivedDataPath "$DERIVED" ${RESOLVE[@]+"${RESOLVE[@]}"} -showBuildTimingSummary build "${SETTINGS[@]}" >"$BUILD_LOG" 2>&1 || status=$?
say "xcodebuild finished (exit $status)"
grep -E '(: error:|^error:|^\*\* |^embedded wheeld)' "$BUILD_LOG" || true
if [ "$status" -ne 0 ]; then
  say "build failed (xcodebuild exited $status); nothing was installed"
  exit 1
fi
APP="$DERIVED/Build/Products/$CONFIG/Wheel.app"

# The dev app is renamed before it is installed: the Dock and Finder take the name from the
# bundle's file name as well as from Info.plist.
if [ "$VARIANT" = dev ]; then
  rm -rf "$DERIVED/Build/Products/$CONFIG/Wheel Dev.app"
  mv "$APP" "$DERIVED/Build/Products/$CONFIG/Wheel Dev.app"
  APP="$DERIVED/Build/Products/$CONFIG/Wheel Dev.app"
  plutil -replace CFBundleDisplayName -string "Wheel Dev" "$APP/Contents/Info.plist"
  plutil -replace CFBundleName -string "Wheel Dev" "$APP/Contents/Info.plist"
  codesign --force --sign "$IDENTITY" "$APP"
  say "renamed and signed"
fi

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
