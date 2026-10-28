#!/usr/bin/env bash
# § 4 Closing a main window (wheel-i7ve.1): the red light and `wheel window close` on a window that
# holds spaces open a sheet on it; Background keeps every space running (its little windows stay,
# owned by none), Close All Processes closes the spaces and their little windows, Cancel changes
# nothing; a window whose last space leaves closes on its own. Drives the real Mac app and the CLI
# against this script's own daemon. With WHEEL_SHOTS set it saves the sheet (run it with
# WHEEL_APPEARANCE=light, then dark).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT
LOOK="${WHEEL_APPEARANCE:-light}"
shot() { [ -z "${WHEEL_SHOTS:-}" ] || { mkdir -p "$WHEEL_SHOTS"; app_post "/window/snapshot?which=sheet&path=$WHEEL_SHOTS/close-$1-$LOOK.png" >/dev/null && echo "     shot $WHEEL_SHOTS/close-$1-$LOOK.png"; }; }

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
IDS=($(mkspaces 4))
S1=${IDS[0]} S2=${IDS[1]} S3=${IDS[2]} S4=${IDS[3]}
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 4)' 20
A="$(st '.windows[0].id')"
B="$(w window new --name Second --space "$S4")"
wait_st '.windows | length == 2' 10
app_post "/windows?action=little&space=$S2" >/dev/null
wait_st '.little_windows | length == 1 and .little_windows[0].open' 5

# The red light asks.
app_post "/windows?action=closebutton&window=$A" >/dev/null
check "the sheet shows on that window" wait_st '.window_close.window == "'"$A"'"' 5
eq "the line counts spaces and panes and the little windows" "$(st .window_close.summary)" "It holds 3 spaces with 3 panes. 1 of them is in a little window."
shot sheet
eq "nothing closed while it asks" "$(st '.windows | length')" "2"

# Cancel changes nothing.
app_post "/windows?action=closeanswer&answer=cancel" >/dev/null
eq "Cancel hides the sheet" "$(st '.window_close')" "null"
eq "and both windows stay" "$(st '.windows | length')" "2"
eq "and so does the little window" "$(st '.little_windows | length')" "1"

# `wheel window close` asks the same; Background keeps the spaces running.
expect_exit 0 "window close shows the sheet and returns" w window close "$A"
check "the sheet shows again" wait_st '.window_close.window == "'"$A"'"' 5
app_post "/windows?action=closeanswer&answer=background" >/dev/null
wait_st '[.windows[] | select(.id=="'"$A"'")] | length == 0' 10
eq "the window is gone" "$(w window ls --json | jq --arg a "$A" '[.windows[] | select(.id==$a)] | length')" "0"
eq "its other spaces are in the background" "$(w window ls --json | jq '.background | length')" "2"
eq "the little window stays open" "$(st '.little_windows | length')" "1"
eq "and belongs to none" "$(w window ls --json | jq -r '.little[0].owner')" "null"
eq "every space still runs in wheeld" "$(w space ls | jq '.spaces | length')" "4"

# Close All Processes ends the spaces and their little windows.
app_post "/windows?action=place&space=$S1&window=$B" >/dev/null
app_post "/windows?action=place&space=$S3&window=$B" >/dev/null
app_post "/windows?action=little&space=$S3" >/dev/null
wait_st '.little_windows | length == 2' 5
app_post "/windows?action=closebutton&window=$B" >/dev/null
check "the sheet shows for the second window" wait_st '.window_close.window == "'"$B"'"' 5
app_post "/windows?action=closeanswer&answer=stop" >/dev/null
wait_st '.windows | length == 0' 10
eq "its spaces are gone from wheel space ls" "$(w space ls | jq '[.spaces[].id | select(. == "'"$S1"'" or . == "'"$S3"'" or . == "'"$S4"'")] | length')" "0"
eq "the other background space is left" "$(w space ls | jq '.spaces | length')" "1"
eq "their little windows closed" "$(st '[.little_windows[] | select(.space == "'"$S3"'")] | length')" "0"
eq "the background little window stays" "$(st '.little_windows | length')" "1"

# A window whose last space leaves closes on its own, by command.
app_post "/windows?action=reopen" >/dev/null
wait_st '.windows | length == 1' 10
C="$(st '.windows[0].id')"
D="$(w window new --name Other)"
wait_st '.windows | length == 2' 10
expect_exit 0 "space move gives the empty window a space" w space move "$S2" "$D"
wait_st '.windows | length == 2' 10
expect_exit 0 "space move takes its last space away" w space move "$S2" "$C"
wait_st '.windows | length == 1' 10
eq "the emptied window is gone from window ls" "$(w window ls --json | jq --arg d "$D" '[.windows[] | select(.id==$d)] | length')" "0"
finish
