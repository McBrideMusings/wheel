#!/bin/bash
# Generates and builds the STT Bench iPhone app for the attached device, then installs it.
set -euo pipefail
B=$(cd "$(dirname "$0")" && pwd)
DEV=00008140-001E1CEE1E81801C
set -a; source ~/.claude/.env; set +a
cd "$B/BenchApp"
xcodegen generate --quiet
xcodebuild -project BenchApp.xcodeproj -scheme BenchApp -configuration Release -destination "id=$DEV" \
  -derivedDataPath "$B/DerivedData-ios" -allowProvisioningUpdates DEVELOPMENT_TEAM="$IOS_DEVELOPMENT_TEAM" build 2>&1 | grep -E "error:|BUILD (SUCCEEDED|FAILED)" | head -20
APP="$B/DerivedData-ios/Build/Products/Release-iphoneos/BenchApp.app"
du -sh "$APP"
xcrun devicectl device install app --device "$DEV" "$APP" 2>&1 | tail -3
