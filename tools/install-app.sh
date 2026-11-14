#!/bin/bash
# Build the GPUI Mac client and install it in /Applications while the running copy keeps running
# (ADR-0040).
#   install-app.sh release   /Applications/Wheel.app (com.piercemakes.wheel, wheeld on 7420)
#   install-app.sh dev       /Applications/Wheel Dev.app (com.piercemakes.wheel.dev, the DEV icon,
#                            the debug wheeld on 7422)
# The Swift Mac app's Release build runs first: clients/gpui/tools/build.py links its objects
# (GhosttyTerminal, WheelKit and what they use) into the GPUI bridge, for both variants. build.py
# then makes and signs the bundle, and tools/swap-app.sh puts it in place: the running app is
# stopped only for the two renames that swap the bundles, then relaunched. wheeld is left running:
# it runs from a copy in Application Support, outside the bundle, so the stop never reaches it, and
# the app offers a server restart itself when the bundled one differs (ADR-0020). Output also goes
# to ~/Library/Logs/wheel/deploy-<variant>.log, which keeps the record when the pane that ran this
# goes away with the app.
set -euo pipefail

VARIANT="${1:-}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
case "$VARIANT" in
  release) DEST=/Applications/Wheel.app ;;
  dev) DEST="/Applications/Wheel Dev.app" ;;
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
SETTINGS=(COMPILATION_CACHING=YES CODE_SIGN_IDENTITY="$IDENTITY" CODE_SIGN_STYLE=Manual PROVISIONING_PROFILE_SPECIFIER=)
[ -n "${IOS_DEVELOPMENT_TEAM:-}" ] && SETTINGS+=(DEVELOPMENT_TEAM="$IOS_DEVELOPMENT_TEAM")

# The Release objects go in their own derived data, the directory build.py reads, so a deploy never
# replaces the Debug app `make build-client` leaves for the checks. `.noindex` keeps Spotlight from
# importing the build products.
DERIVED="$ROOT/DerivedData-deploy-release.noindex"

( cd "$ROOT" && xcodegen generate --spec apple/project.yml --project apple )
BUILD_LOG="$LOG_DIR/deploy-$VARIANT-xcodebuild.log"
say "building the Swift Release objects; xcodebuild's output is in $BUILD_LOG"
# The generated workspace records the package versions once resolved; reading them back skips a
# network round trip per build. A project that was never resolved resolves on its first build.
RESOLVE=()
[ -f "$ROOT/apple/Wheel.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved" ] \
  && RESOLVE=(-disableAutomaticPackageResolution)
status=0
xcodebuild -project "$ROOT/apple/Wheel.xcodeproj" -scheme Wheel -configuration Release \
  -destination platform=macOS -derivedDataPath "$DERIVED" ${RESOLVE[@]+"${RESOLVE[@]}"} -showBuildTimingSummary build "${SETTINGS[@]}" >"$BUILD_LOG" 2>&1 || status=$?
say "xcodebuild finished (exit $status)"
grep -E '(: error:|^error:|^\*\* |^embedded wheeld)' "$BUILD_LOG" || true
if [ "$status" -ne 0 ]; then
  say "build failed (xcodebuild exited $status); nothing was installed"
  exit 1
fi

GPUI_LOG="$LOG_DIR/deploy-$VARIANT-build.log"
say "building the GPUI client ($VARIANT); the build's output is in $GPUI_LOG"
# build.py's last line is the bundle it made; everything else it and its tools print is in the log.
if ! WHEEL_NATIVE_PRODUCTS="$DERIVED/Build/Products/Release" python3 "$ROOT/clients/gpui/tools/build.py" "$VARIANT" >"$GPUI_LOG" 2>&1; then
  tail -5 "$GPUI_LOG"
  say "build failed (see $GPUI_LOG); nothing was installed"
  exit 1
fi
grep -E '^(warning: no Apple Development|embedded wheeld)' "$GPUI_LOG" || true
APP="$(tail -1 "$GPUI_LOG")"
[ -d "$APP" ] || { say "build.py did not name the bundle it made (last line: $APP)"; exit 1; }
say "built $APP"

bash "$ROOT/tools/swap-app.sh" "$APP" "$DEST"
