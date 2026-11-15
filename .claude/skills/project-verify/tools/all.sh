#!/usr/bin/env bash
# Runs every check script in turn and prints each one's last line. Exits non-zero if any failed.
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
failed=0
app="${WHEEL_DERIVED_DATA:-$here/../../../../DerivedData}/Build/Products/Debug/Wheel.app"
for s in daemon spaces web mcp-shared wheelhouse-agents builtins runner usage-review browser settings usage usage-strip control pairing windows menubar background-spaces window-commands window-close drag pane-space-drag popped-drop little-window tab-click space-menu server pairing-window status hotkeys pane-kind-menu mac-builtins palette-runner shortcut-sheet terminal-look permissions companion unsend ios-web ios-turns-terminal processes; do
  printf '%-12s ' "$s"
  # The Mac checks (every script that sources mac-lib.sh) drive the built app, and the iOS ones the
  # simulator build; without one they are skipped, not failed.
  if grep -q 'mac-lib\.sh' "$here/$s.sh" && [ ! -d "$app" ]; then echo "skipped (no Mac app at $app)"; continue; fi
  if case "$s" in ios-*) [ ! -d "${WHEEL_IOS_APP:-}" ] ;; *) false ;; esac; then echo "skipped (no iOS app in WHEEL_IOS_APP)"; continue; fi
  out="$(bash "$here/$s.sh" 2>&1)"; code=$?
  printf '%s\n' "$(printf '%s\n' "$out" | tail -1)"
  if [ "$code" != 0 ]; then
    failed=$((failed + 1))
    printf '%s\n' "$out" | grep '^FAIL' | sed 's/^/    /'
  fi
done
[ "$failed" = 0 ]
