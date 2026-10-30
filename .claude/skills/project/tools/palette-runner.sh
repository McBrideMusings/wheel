#!/usr/bin/env bash
# § 2c-builtins, the command palette's Run rows: a space whose wheel.toml has a [runner] table gets
# one `Run <name>` row per entry, and running one splits a terminal pane running its command.
# Drives the real Mac app through its debug server against this script's own daemon.
# WHEEL_SHOTS=<dir> keeps the window snapshot (set WHEEL_APPEARANCE=light|dark for the theme).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"
LOOK="${WHEEL_APPEARANCE:-light}"

start_daemon --state-file "$SCR/spaces.json"
mkdir -p "$SCR/proj"
printf '[runner]\nhello = "echo palette-says-$((6*7))"\n' >"$SCR/proj/wheel.toml"
w space new "$SCR/proj" >/dev/null
S="$(w space ls | jget spaces.0.id)"
prefs_reset
launch_app
wait_st '.windows | length == 1' 20
wait_st '.spaces | length >= 1' 20

palette() { app_post "/palette?$1" >/dev/null; }
panes() { w space ls | jq '[.spaces[0] | .. | objects | select(.kind? == "terminal")] | length'; }

BEFORE="$(panes)"
palette "action=open&mode=root"
check "the palette lists Run hello" wait_st '[.palette.rows[] | select(.kind == "action" and .title == "Run hello")] | length == 1' 10
palette "action=type&text=run%20hello"
check "typing the name leaves Run hello selected" wait_st '.palette.rows[.palette.selected].title == "Run hello"' 5
app_post "/window/snapshot?path=$SHOTS/palette-runner-$LOOK.png" >/dev/null && echo "     shot $SHOTS/palette-runner-$LOOK.png"

palette "action=run"
check "running it closes the palette" wait_st '.palette.open == false' 5
gained() { [ "$(panes)" -gt "$BEFORE" ]; }
check "the space gained a terminal pane" poll 10 gained
T="$(w space ls | jq -r '[.spaces[0] | .. | objects | select(.kind? == "terminal") | .id] | last')"
check "the palette's Run row focuses the new terminal pane" wait_st ".spaces[] | select(.id == \"$S\") | .focus == \"$T\"" 5
expect_exit 0 "the terminal ran the runner's command" w term read "$T" --until "palette-says-42" --timeout 10

# The palette's Split Right command is a person's action: it focuses the new pane.
F0="$(st --arg s "$S" '.spaces[] | select(.id == $s) | .focus')"
palette "action=open&mode=root"
palette "action=type&text=split%20right"
check "typing split right leaves Split Right selected" wait_st '.palette.rows[.palette.selected].title == "Split Right"' 5
palette "action=run"
check "running Split Right focuses a new pane" wait_st ".spaces[] | select(.id == \"$S\") | .focus != \"$F0\" and .focus != \"$T\"" 10

APPLOG="$(st .log_path)"
check "the app log lists the runners" grep -q "command palette: 1 runners listed: hello" "$APPLOG"
check "and runs the action" grep -q "run action action:run-hello" "$APPLOG"
check "and names the space" grep -q "runner hello: run in space $S" "$APPLOG"
finish
