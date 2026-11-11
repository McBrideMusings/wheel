#!/bin/bash
# Build the Mac app and install it in /Applications while the running copy keeps running.
#   install-app.sh release   Release build, installed as /Applications/Wheel.app
#   install-app.sh dev       Debug build with the .dev bundle id and the AppIconDev icon,
#                            installed as /Applications/Wheel Dev.app
# tools/swap-app.sh puts the built bundle in place: the running app is stopped only for the two
# renames that swap the bundles, then relaunched. wheeld is left running: it runs from a copy in
# Application Support, outside the bundle, so the stop never reaches it, and the app offers a
# server restart itself when the bundled one differs (ADR-0020). Output also goes to
# ~/Library/Logs/wheel/deploy-<variant>.log, which keeps the record when the pane that ran this
# goes away with the app.
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
# Swift compilation caching is keyed by file content, so a fresh DerivedData (a new worktree, a cleared
# build) reuses every module compiled before.
SETTINGS+=(COMPILATION_CACHING=YES CODE_SIGN_IDENTITY="$IDENTITY" CODE_SIGN_STYLE=Manual PROVISIONING_PROFILE_SPECIFIER=)
[ -n "${IOS_DEVELOPMENT_TEAM:-}" ] && SETTINGS+=(DEVELOPMENT_TEAM="$IOS_DEVELOPMENT_TEAM")

# Each variant has its own derived data, so a deploy never replaces the Debug app `make
# build-client` leaves for the checks.
# `.noindex` keeps Spotlight from importing the build products.
DERIVED="$ROOT/DerivedData-deploy-$VARIANT.noindex"

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

bash "$ROOT/tools/swap-app.sh" "$APP" "$DEST"
