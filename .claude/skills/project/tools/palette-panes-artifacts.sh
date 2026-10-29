#!/usr/bin/env bash
# § The command palette's pane and artifact rows (wheel-6d7g): the palette lists the panes of every
# space and the active space's artifacts; running a pane row focuses that pane (in its own space),
# running an artifact row adds a web pane right of the focused pane. Drives the real Mac app through
# its debug server against this script's own daemon.
# WHEEL_SHOTS=<dir> keeps the window snapshots (set WHEEL_APPEARANCE=light|dark for the theme).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"
LOOK="${WHEEL_APPEARANCE:-light}"

start_daemon --state-file "$SCR/spaces.json"
mkdir -p "$SCR/one" "$SCR/two"
w space new "$SCR/one" >/dev/null
w space new "$SCR/two" >/dev/null
S1="$(w space ls | jget spaces.0.id)"
S2="$(w space ls | jget spaces.1.id)"
w artifact new status --pane --space "$S2" >/dev/null
printf '<!doctype html><title>status</title><p>palette-artifact</p>' >"$SCR/two/.wheel/artifacts/status/index.html" 2>/dev/null
P1="$(w space ls | jq -r --arg s "$S1" '[.spaces[] | select(.id == $s)][0] | [.. | objects | select(.id? and .kind?) | .id] | first')"; w pane split "$P1" right >/dev/null
prefs_reset
launch_app
wait_st '.windows | length == 1' 20
wait_st '.spaces | length >= 2' 20
w space focus "$S2" >/dev/null 2>&1 || true

palette() { app_post "/palette?$1" >/dev/null; }
settle() { sleep 0.6; }
shot() { app_post "/window/snapshot?path=$SHOTS/palette-$1-$LOOK.png" >/dev/null && echo "     shot $SHOTS/palette-$1-$LOOK.png"; }
focused() { st --arg s "$1" '[.spaces[] | select(.id == $s)][0].focus'; }

palette "action=open&mode=root"
check "the palette lists a pane row" wait_st '[.palette.rows[] | select(.kind == "pane")] | length >= 2' 10
check "the palette lists the status artifact" wait_st '[.palette.rows[] | select(.kind == "artifact" and .id == "artifact:project/status")] | length == 1' 10
st -c '[.palette.rows[] | .kind + ":" + .title + ":" + .detail]'
shot all

palette "action=type&text=status"
settle
eq "typing status leaves the artifact row" "$(st -c '[.palette.rows[] | select(.kind == "artifact") | .title]')" '["status"]'
shot artifact

# Running a pane row of another space focuses it.
TARGET="$(w space ls | jq -r --arg s "$S1" '[.spaces[] | select(.id == $s)][0] | [.. | objects | select(.id? and .kind?) | .id] | last')"
palette "action=type&text="
settle
IDX="$(st --arg t "pane:$TARGET" '[.palette.rows[] | .id] | index($t)')"
palette "action=select&index=$IDX"
palette "action=run"
check "running a pane row closes the palette" wait_st '.palette.open == false' 5
settle
eq "the pane is the focused pane of its space" "$(focused "$S1")" "$TARGET"

# Running the artifact row splits right of the focused pane and shows it.
w space focus "$S2" >/dev/null 2>&1 || true
settle
BEFORE="$(w space ls | jq --arg s "$S2" '[.spaces[] | select(.id == $s)][0] | [.. | objects | select(.kind? == "web")] | length')"
palette "action=open&mode=root"
palette "action=type&text=status"
settle
palette "action=run"
webs() { [ "$(w space ls | jq --arg s "$S2" '[.spaces[] | select(.id == $s)][0] | [.. | objects | select(.kind? == "web")] | length')" -gt "$BEFORE" ]; }
check "the space gained a web pane" poll 10 webs
shot after

finish
