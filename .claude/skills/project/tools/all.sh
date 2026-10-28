#!/usr/bin/env bash
# Runs every check script in turn and prints each one's last line. Exits non-zero if any failed.
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
failed=0
app="${WHEEL_DERIVED_DATA:-$here/../../../../DerivedData}/Build/Products/Debug/Wheel.app"
for s in daemon spaces web builtins runner browser settings usage control pairing windows menubar background-spaces window-commands drag little-window tab-click space-menu server pairing-window status hotkeys mac-builtins palette-runner ios-web ios-turns-terminal; do
  printf '%-12s ' "$s"
  # The Mac checks drive the built app; without one they are skipped, not failed.
  if case "$s" in windows|menubar|background-spaces|window-commands|drag|little-window|tab-click|space-menu|server|pairing-window|status|hotkeys|mac-builtins|palette-runner) [ ! -d "$app" ] ;; ios-web|ios-turns-terminal) [ ! -d "${WHEEL_IOS_APP:-}" ] ;; *) false ;; esac; then echo "skipped (no build: Mac app at $app, iOS app in WHEEL_IOS_APP)"; continue; fi
  out="$(bash "$here/$s.sh" 2>&1)"; code=$?
  printf '%s\n' "$(printf '%s\n' "$out" | tail -1)"
  if [ "$code" != 0 ]; then
    failed=$((failed + 1))
    printf '%s\n' "$out" | grep '^FAIL' | sed 's/^/    /'
  fi
done
[ "$failed" = 0 ]
