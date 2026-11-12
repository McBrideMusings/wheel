#!/usr/bin/env bash
# wheel-xkuy: a terminal that is not on screen but still holds the keyboard does not pull the
# window back to its space (paneFocused), while a visible pane of another window is reported in that
# window's view (`wheel client ls`).
# Drives the real Mac app through its debug server against this script's own daemon.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
IDS=($(mkspaces 3))
A=${IDS[0]} B=${IDS[1]} C=${IDS[2]}
pane_of() { w space ls | jq -r --arg s "$1" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id'; }
PA="$(pane_of "$A")" PB="$(pane_of "$B")" PC="$(pane_of "$C")"
for p in "$PA" "$PB" "$PC"; do w pane kind "$p" terminal >/dev/null; done

launch_app
wait_st '.windows | length == 1' 20
app_post "/focus?space=$A" >/dev/null
sleep 0.5
app_post "/focus?space=$B" >/dev/null
sleep 0.5
# A's terminal is hidden and reports the keyboard (what isFocused turning true does).
app_post "/pane/focused?pane=$PA" >/dev/null
sleep 2
eq "a hidden terminal's focus leaves B shown in the window" "$(st '.windows[0].active')" "$B"
check "the app logged that it ignored the hidden terminal" poll 5 grep -q "pane focus $PA: ignored" "$INST/WheelMac.log"

# The app reported the window's view to wheeld: B, and B's focused pane (wheeld keeps no active space).
view_of() { w client ls --json | jq -r --arg s "$1" '[.[].windows[] | select(.space == $s)] | length'; }
check "wheeld lists a window of the app showing B" poll 5 test "$(view_of "$B")" -ge 1

# Two windows: each reports its own space; a click in the visible pane of the second shows C there only.
app_post "/windows?action=new&spaces=$C" >/dev/null
wait_st '.windows | length == 2' 10
app_post "/focus?space=$B" >/dev/null
sleep 0.5
app_post "/pane/focused?pane=$PC" >/dev/null
sleep 1
check "wheeld lists a window showing C, with C's pane" poll 5 test "$(w client ls --json | jq -r --arg s "$C" --arg p "$PC" '[.[].windows[] | select(.space == $s and .pane == $p)] | length')" -ge 1
eq "...and one still showing B" "$(view_of "$B")" "1"
finish
