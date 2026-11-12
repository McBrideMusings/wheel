#!/usr/bin/env bash
# Builds the WheelPhone app for the simulator, then runs the iOS check scripts with
# WHEEL_IOS_APP pointing at it. Pass script names (ios-web ...) to run fewer than all.
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../../.." && pwd)"
DD="${WHEEL_IOS_DERIVED_DATA:-$root/DerivedData-ios}"

xcodegen generate --spec "$root/apple/project.yml" --project "$root/apple" || exit 1
xcodebuild -project "$root/apple/Wheel.xcodeproj" -scheme WheelPhone -configuration Debug \
  -destination 'generic/platform=iOS Simulator' -derivedDataPath "$DD" build || exit 1
export WHEEL_IOS_APP="$DD/Build/Products/Debug-iphonesimulator/Wheel.app"
[ -d "$WHEEL_IOS_APP" ] || { echo "no app at $WHEEL_IOS_APP" >&2; exit 1; }

scripts=("$@")
[ "${#scripts[@]}" -gt 0 ] || scripts=(ios-web ios-turns-terminal ios-pairing ios-status ios-new-space)
failed=0
for s in "${scripts[@]}"; do
  printf '%-20s ' "$s"
  out="$(env -u WHEEL_SPACE -u WHEEL_PANE bash "$here/$s.sh" 2>&1)"; code=$?
  printf '%s\n' "$(printf '%s\n' "$out" | tail -1)"
  if [ "$code" != 0 ]; then
    failed=$((failed + 1))
    printf '%s\n' "$out" | grep '^FAIL' | sed 's/^/    /'
  fi
done
[ "$failed" = 0 ]
