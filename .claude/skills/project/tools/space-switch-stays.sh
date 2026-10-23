#!/usr/bin/env bash
# wheel-xkuy: a terminal that is not on screen but still holds the keyboard does not pull the
# window back to its space (paneFocused), while a visible pane of another window still does.
# Drives the real Mac app through its debug server against this script's own daemon.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
IDS=($(mkspaces 3))
A=${IDS[0]} B=${IDS[1]} C=${IDS[2]}
pane_of() { w space ls | jq -r --arg s "$1" '.spaces[] | select(.id==$s) | .focus'; }
PA="$(pane_of "$A")" PB="$(pane_of "$B")" PC="$(pane_of "$C")"
for p in "$PA" "$PB" "$PC"; do w pane kind "$p" terminal >/dev/null; done

launch_app
wait_st '.windows | length == 1' 20
app_post "/focus?space=$A" >/dev/null
sleep 0.5
app_post "/focus?space=$B" >/dev/null
sleep 0.5
eq "B is the daemon's active space after the switch" "$(w space ls | jq -r .active)" "$B"
# A's terminal is hidden and reports the keyboard (what isFocused turning true does).
app_post "/pane/focused?pane=$PA" >/dev/null
sleep 2
eq "a hidden terminal's focus leaves B active in the daemon" "$(w space ls | jq -r .active)" "$B"
eq "a hidden terminal's focus leaves B shown in the window" "$(st '.windows[0].active')" "$B"
check "the app logged that it ignored the hidden terminal" poll 5 grep -q "pane focus $PA: ignored" "$INST/WheelMac.log"

# Two windows: a click in the visible pane of the second makes its space the daemon's active one.
app_post "/windows?action=new&spaces=$C" >/dev/null
wait_st '.windows | length == 2' 10
app_post "/focus?space=$B" >/dev/null
sleep 0.5
eq "before the click B is the daemon's active space" "$(w space ls | jq -r .active)" "$B"
app_post "/pane/focused?pane=$PC" >/dev/null
sleep 1
eq "a visible pane of another window makes its space active" "$(w space ls | jq -r .active)" "$C"
finish
