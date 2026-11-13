#!/usr/bin/env bash
# A terminal shown by two windows on one session keeps drawing in the window left after the other
# closes (wheel-s0aa). Each window's view of a terminal has its own surface and session; the app
# gives the daemon's output to every surface, and a surface made after attach gets a replay.
# Drives the real Mac app against this script's own daemon and reads each surface's visible text
# from /state (`panes[].surfaces`).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
S="$(mkspaces 1)"
P="$(w space ls | jq -r --arg s "$S" '.spaces[] | select(.id==$s) | .. | objects | select(.node=="pane") | .id' | head -1)"
w pane start "$P" terminal >/dev/null
launch_app
# surf: the visible text of every surface of the space's terminal pane, as a JSON array.
surf='[.spaces[]?.panes[]?, .panes[]? | select(.term != null)][0].surfaces'
check "one window draws the terminal with one surface" wait_st "($surf | length) == 1" 20
w term send "$P" "echo first-$$" --enter >/dev/null
check "the one surface shows output" wait_st "($surf | map(test(\"first-$$\")) | all)" 10

A="$(st '.windows[0].id')"
SESSION="$(w window ls --json | jq -r --arg a "$A" '.windows[] | select(.id==$a) | .session')"
B="$(app_post "/windows?action=new&session=$SESSION" | jq -r .window)"
check "a second window on the same session" wait_st '.windows | length == 2' 10
check "the terminal has a surface per window" wait_st "($surf | length) == 2" 10
check "the second surface is filled by a replay" wait_st "($surf | map(test(\"first-$$\")) | all)" 10
w term send "$P" "echo second-$$" --enter >/dev/null
check "both surfaces show new output" wait_st "($surf | map(test(\"second-$$\")) | all)" 10

app_post "/windows?action=closebutton&window=$B" >/dev/null
wait_st '.window_close != null or (.windows | length == 1)' 5
[ "$(st '.window_close')" = "null" ] || app_post "/windows?action=closeanswer&answer=background" >/dev/null
check "the second window closes" wait_st '.windows | length == 1' 10
check "one surface is left" wait_st "($surf | length) == 1" 10
w term send "$P" "echo third-$$" --enter >/dev/null
check "the window left still draws new output" wait_st "($surf | map(test(\"third-$$\")) | all)" 10
[ "$FAIL" = 0 ] || { echo "state:"; st "$surf"; st '.windows'; }
finish
