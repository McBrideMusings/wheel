#!/usr/bin/env bash
# The pane header's zoom button (wheel-h3z5.17): /state lists it per pane, a press zooms and
# unzooms, the ⋯ menu no longer offers zoom, and a one-pane space shows it disabled.
# WHEEL_APPEARANCE=light|dark picks the look; WHEEL_SHOTS=<dir> keeps zoom-button-<state>-<look>.png.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
mkspaces 1 >/dev/null
prefs_reset

P1="$(w status --json | jq -r '[.. | objects | select(.kind? != null) | .id][0]')"
w pane kind "$P1" terminal >/dev/null
launch_app
check "the app connects" wait_st '.connection == "connected" and (.windows | length) == 1' 20
LOOK="${WHEEL_APPEARANCE:-light}"
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"; mkdir -p "$SHOTS"
btn() { st ".panes[\"$P1\"].header_buttons[0] | $1"; }

eq "one pane: the zoom button is listed, disabled" "$(btn '[.id, .enabled, .on] | @csv')" '"zoom",false,false'
eq "one pane: its title" "$(btn .title)" "Zoom Pane (⇧⌘↩)"
refused="$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $(TOKEN)" "http://127.0.0.1:$DEBUG_PORT/pane/button?pane=$P1&button=zoom")"
eq "one pane: a press is refused" "$refused" 409

P2="$(w pane split "$P1" right)"
w pane kind "$P2" terminal >/dev/null
check "two panes: the button is enabled" wait_st ".panes[\"$P1\"].header_buttons[0].enabled == true"
eq "the ⋯ menu has no zoom item" "$(st "[.panes[\"$P1\"].menu[].id] | index(\"zoom\")")" null
eq "unzoomed symbol" "$(btn .symbol)" arrow.up.left.and.arrow.down.right
app_post "/focus?pane=$P1" >/dev/null
sleep 1
app_post "/window/snapshot?path=$SHOTS/zoom-button-unzoomed-$LOOK.png" >/dev/null && echo "shot $SHOTS/zoom-button-unzoomed-$LOOK.png"

app_post "/pane/button?pane=$P1&button=zoom" >/dev/null
check "a press zooms the pane" wait_st ".zoomed == \"$P1\""
eq "zoomed: title" "$(btn .title)" "Unzoom Pane (⇧⌘↩)"
eq "zoomed: symbol" "$(btn .symbol)" arrow.down.right.and.arrow.up.left
eq "zoomed: on" "$(btn .on)" true
sleep 1
app_post "/window/snapshot?path=$SHOTS/zoom-button-zoomed-$LOOK.png" >/dev/null && echo "shot $SHOTS/zoom-button-zoomed-$LOOK.png"

app_post "/pane/button?pane=$P1&button=zoom" >/dev/null
check "a second press unzooms" wait_st ".zoomed == null"
eq "unzoomed again: on" "$(btn .on)" false

# The same header in a little window: the space (two panes) opens there, the button zooms it.
S1="$(w space ls | jq -r '.spaces[0].id')"
app_post "/space/menu?space=$S1&item=little" >/dev/null
check "the space opens in a little window" wait_st '.little_windows | length == 1' 10
eq "and it is open" "$(st '.little_windows[0].open')" true
sleep 1.5
app_post "/pane/button?pane=$P2&button=zoom" >/dev/null
check "in the little window a press zooms the pane" wait_st ".panes[\"$P2\"].zoomed == true" 5
eq "and the button reads on" "$(st ".panes[\"$P2\"].header_buttons[0].on")" true
sleep 1
LW="$(st '.little_windows[0].id' | cut -c1-8)"
app_post "/window/snapshot?which=little&window=$LW&path=$SHOTS/zoom-button-little-zoomed-$LOOK.png" >/dev/null && echo "shot $SHOTS/zoom-button-little-zoomed-$LOOK.png"
finish
