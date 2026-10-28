#!/usr/bin/env bash
# Builds the WheelPhone app for a real device, installs it and launches it.
# Needs WHEEL_IOS_DEVICE (the device's CoreDevice id or UDID; `xcrun devicectl list devices`
# prints them) and IOS_DEVELOPMENT_TEAM, both read from ~/.claude/.env when not already set.
set -u
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
[ -f "$HOME/.claude/.env" ] && { set -a; . "$HOME/.claude/.env"; set +a; }
BUNDLE=com.piercemakes.wheel.phone
DEVICE="${WHEEL_IOS_DEVICE:-}"
[ -n "$DEVICE" ] || { echo "set WHEEL_IOS_DEVICE to the phone's device id (xcrun devicectl list devices)" >&2; exit 2; }
[ -n "${IOS_DEVELOPMENT_TEAM:-}" ] || { echo "set IOS_DEVELOPMENT_TEAM (the Apple team id)" >&2; exit 2; }
DD="${WHEEL_IOS_DERIVED_DATA:-$root/DerivedData-phone}"

xcodegen generate --spec "$root/apple/project.yml" --project "$root/apple" || exit 1
xcodebuild -project "$root/apple/Wheel.xcodeproj" -scheme WheelPhone -configuration Debug \
  -destination "id=$DEVICE" -derivedDataPath "$DD" -allowProvisioningUpdates \
  DEVELOPMENT_TEAM="$IOS_DEVELOPMENT_TEAM" build || exit 1
xcrun devicectl device install app --device "$DEVICE" "$DD/Build/Products/Debug-iphoneos/Wheel.app" || exit 1
out="$(xcrun devicectl device process launch --terminate-existing --device "$DEVICE" "$BUNDLE" 2>&1)"; code=$?
printf '%s\n' "$out"
if [ "$code" != 0 ]; then
  case "$out" in
    *"could not be unlocked"*)
      echo "installed, but the launch failed: the phone is locked. Unlock it and run again, or open Wheel by hand." >&2 ;;
  esac
  exit "$code"
fi
