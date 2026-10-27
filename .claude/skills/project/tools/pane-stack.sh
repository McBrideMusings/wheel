#!/usr/bin/env bash
# § 4 A space's pane count as stacked layers on its tab and sidebar row: spaces holding 1, 2, 3 and
# 5 panes draw 1, 2, 3 and 4 layers (`/state` spaces[].stack.layers), `wheel pane split` adds a
# layer without a relaunch, and the window snapshots (light and dark) are saved for a look.
# WHEEL_SHOTS=<dir> keeps the PNGs; the shape of the drawing is checked by eye.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
IDS=($(mkspaces 4))
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 4)' 20

layers() { st --arg s "$1" '.spaces[] | select(.id==$s) | .stack.layers'; }
glyph() { st --arg s "$1" '.spaces[] | select(.id==$s) | .stack.glyph'; }
grow() {
  local space="$1" want="$2" have
  have="$(w space ls | jq --arg s "$space" '[.spaces[] | select(.id==$s) | .. | objects | select(has("kind"))] | length')"
  while [ "$have" -lt "$want" ]; do
    w pane split "$(w space ls | jq -r --arg s "$space" '[.spaces[] | select(.id==$s) | .. | objects | select(has("kind")) | .id] | .[0]')" right >/dev/null
    have=$((have + 1))
  done
}

grow "${IDS[1]}" 2; grow "${IDS[2]}" 3; grow "${IDS[3]}" 5
wait_st "(.spaces | length) == 4" 5
sleep 1
eq "1 pane draws 1 layer" "$(layers "${IDS[0]}")" 1
eq "2 panes draw 2 layers" "$(layers "${IDS[1]}")" 2
eq "3 panes draw 3 layers" "$(layers "${IDS[2]}")" 3
eq "5 panes draw 4 layers" "$(layers "${IDS[3]}")" 4

eq "the front layer shows the first pane's kind" "$(glyph "${IDS[0]}")" "rectangle.dashed"

grow "${IDS[0]}" 2
poll 20 sh -c "[ \"\$(curl -sf -H 'Authorization: Bearer $(TOKEN)' http://127.0.0.1:$DEBUG_PORT/state | jq -r --arg s '${IDS[0]}' '.spaces[] | select(.id==\$s) | .stack.layers')\" = 2 ]"
eq "a split adds a layer without a relaunch" "$(layers "${IDS[0]}")" 2

OUT="${WHEEL_SHOTS:-$SCR}"; mkdir -p "$OUT"
for mode in light dark; do
  kill_app; DEBUG_PORT=""; WHEEL_APPEARANCE=$mode launch_app
  wait_st '.windows | length == 1 and (.windows[0].spaces | length == 4)' 20
  sleep 1
  app_post "/window/snapshot?path=$OUT/pane-stack-$mode.png" >/dev/null
  check "$mode snapshot written" test -s "$OUT/pane-stack-$mode.png"
  echo "snapshot: $OUT/pane-stack-$mode.png"
done
finish
