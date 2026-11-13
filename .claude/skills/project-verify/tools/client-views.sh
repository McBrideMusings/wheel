#!/usr/bin/env bash
# wheel-zpa4.1.4: each client keeps its own view (ADR-0021). wheeld holds no active space and no
# focused pane; the Mac app reports each window's space and pane, `wheel client ls` lists them,
# and a space switch in one window moves no other window. A CLI command with no space takes the
# calling pane's space, or fails naming --space.
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

check "the listing carries no active space" test "$(w space ls | jq 'has("active")')" = false
check "a space carries no focus" test "$(w space ls | jq '.spaces[0] | has("focus")')" = false

launch_app
wait_st '.windows | length == 1' 20
app_post "/windows?action=new&spaces=$C" >/dev/null
wait_st '.windows | length == 2' 10
W1="$(st '.windows[0].id')" W2="$(st '.windows[1].id')"
app_post "/focus?space=$A" >/dev/null
sleep 1

views() { w client ls --json | jq -c '[.[] | select(.client == "mac") | .windows[] | {window, space, pane}] | sort_by(.window)'; }
view_space() { w client ls --json | jq -r --arg k "$1" '[.[] | .windows[] | select(.window == $k)][0].space'; }
check "client ls lists a mac client" poll 5 test "$(w client ls --json | jq '[.[] | select(.client == "mac")] | length')" -ge 1
eq "window 1 reports the space it shows" "$(view_space "$W1")" "$(st --arg w "$W1" '.windows[] | select(.id==$w) | .active')"
eq "window 2 reports C" "$(view_space "$W2")" "$C"
check "the human-readable listing names the window" bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' client ls | grep -q \"window $W2 .*space $C\""

# A switch in window 1 changes window 1's report and nothing in window 2.
before2="$(view_space "$W2")"
app_post "/focus?space=$B" >/dev/null
check "window 1 reports B after the switch" poll 5 test "$(view_space "$W1")" = "$B"
eq "window 2 still reports C" "$(view_space "$W2")" "$before2"
eq "window 2 still shows C" "$(st --arg w "$W2" '.windows[] | select(.id==$w) | .active')" "$C"

# A CLI command with no space takes the calling pane's space; outside a pane it fails naming --space.
D="$SCR/artifacts-here"; mkdir -p "$D"
OUT="$(env -u WHEEL_SPACE WHEEL_PANE="$PB" "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$INST/token" artifact new x 2>&1)"
check "artifact new from B's pane lands in B's directory" bash -c "[[ '$OUT' == $(w space ls | jq -r --arg s "$B" '.spaces[] | select(.id==$s) | .cwd')/* ]]"
ERR="$(env -u WHEEL_SPACE -u WHEEL_PANE "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$INST/token" artifact new y 2>&1)"; RC=$?
check "artifact new outside a pane fails" test "$RC" -ne 0
check "...naming --space" bash -c "[[ '$ERR' == *--space* ]]"

# wheel space focus is a request every client shows; it needs no daemon state.
w space focus "$A" >/dev/null
check "space focus shows the space in the window that holds it" poll 5 test "$(view_space "$W1")" = "$A"
finish
