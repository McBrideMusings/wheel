#!/bin/bash
# Build the Swift Mac app's Release objects (GhosttyTerminal, WheelKit, WheelUI and what they use)
# that clients/gpui/tools/build.py links into the GPUI bridge, into the checkout's
# DerivedData-deploy-release.noindex, so the bridge always links this checkout's Swift.
#   swift-objects.sh <log>   xcodebuild's output goes to <log>; errors are repeated on stdout
set -euo pipefail

LOG="${1:?usage: swift-objects.sh <log>}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$(dirname "$LOG")"

# Sign with the Apple Development certificate by its hash, so macOS keeps the permissions it
# granted the app. With no such certificate the app is signed ad hoc.
IDENTITY="$("$ROOT/tools/signing-identity")"
[ -n "$IDENTITY" ] || IDENTITY=-
# Swift compilation caching is keyed by file content, so a fresh DerivedData (a new worktree, a cleared
# build) reuses every module compiled before.
SETTINGS=(COMPILATION_CACHING=YES CODE_SIGN_IDENTITY="$IDENTITY" CODE_SIGN_STYLE=Manual PROVISIONING_PROFILE_SPECIFIER=)
[ -n "${IOS_DEVELOPMENT_TEAM:-}" ] && SETTINGS+=(DEVELOPMENT_TEAM="$IOS_DEVELOPMENT_TEAM")

# Their own derived data, so this never replaces the Debug app `make build-client` leaves for the
# checks. `.noindex` keeps Spotlight from importing the build products.
DERIVED="$ROOT/DerivedData-deploy-release.noindex"

( cd "$ROOT" && xcodegen generate --spec apple/project.yml --project apple )
# The generated workspace records the package versions once resolved; reading them back skips a
# network round trip per build. A project that was never resolved resolves on its first build.
RESOLVE=()
[ -f "$ROOT/apple/Wheel.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved" ] \
  && RESOLVE=(-disableAutomaticPackageResolution)
status=0
xcodebuild -project "$ROOT/apple/Wheel.xcodeproj" -scheme Wheel -configuration Release \
  -destination platform=macOS -derivedDataPath "$DERIVED" ${RESOLVE[@]+"${RESOLVE[@]}"} -showBuildTimingSummary build "${SETTINGS[@]}" >"$LOG" 2>&1 || status=$?
grep -E '(: error:|^error:|^\*\* |^embedded wheeld)' "$LOG" || true
exit "$status"
