#!/usr/bin/env bash
# § Four chat panes in a window too narrow for their minimum width never draw over each other.
# The grid shares a too-small width among the panes (GridLayout.minimumPane), and a pane's view
# is held to the frame it is given even when its footer row (context meter, model, mode) is
# wider than that. The check sets the window to 860 pt, reads each pane's drawn frame from
# /state and requires the frames not to intersect, then draws the window to $WHEEL_SHOTS/pane-overlap.png.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS" "$SCR/a"
printf '[agent.claude]\ncommand = "%s"\n' "$FAKE_AGENT" >"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"
prefs_reset

A="$(w space new "$SCR/a")"
sp() { w space ls | jq -r --arg s "$A" '[.spaces[] | select(.id==$s) | .root | .. | objects | select(.node? == "pane") | .id] | .[]'; }
while [ "$(sp | wc -l)" -lt 4 ]; do
  w pane split "$(sp | tail -1)" right >/dev/null
done
for p in $(sp); do w pane start "$p" agent >/dev/null; done

launch_app
check "the app connects" wait_st '.connection == "connected" and (.windows | length) == 1' 20
WIN="$(st '.windows[0].id')"
app_post "/windows?action=frame&window=$WIN&x=40&y=100&w=860&h=900" >/dev/null
app_post "/focus?space=$A" >/dev/null
check "four agent panes are drawn" wait_st '[.panes[] | select(.kind == "agent")] | length == 4' 20
sleep 2

# Every pair of drawn frames: the overlap along x must be zero (side by side, so x decides).
OVER="$(st '[.panes[] | select(.kind == "agent" and .frame != null) | .frame] | sort_by(.x) as $f
  | [range(0; ($f | length) - 1) | ($f[.].x + $f[.].w) - $f[. + 1].x] | map(select(. > 0.5)) | length')"
eq "no two panes' frames overlap" "$OVER" "0"
app_post "/window/snapshot?path=$SHOTS/pane-overlap.png" >/dev/null && echo "shot $SHOTS/pane-overlap.png"
finish
