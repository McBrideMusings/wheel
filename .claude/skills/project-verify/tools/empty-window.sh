#!/usr/bin/env bash
# § 6 A window with no spaces (wheel-tn7z): it draws no sidebar and shows the command palette in
# place, opened by itself once the window is the focused one; a space picked there comes to that
# window rather than the app jumping to the space's own. With WHEEL_SHOTS set it saves the empty
# window, named after WHEEL_APPEARANCE.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
IDS=($(mkspaces 2))
S1=${IDS[0]}
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 2)' 20
A="$(st '.windows[0].id')"
MLOG="$INST/WheelMac.log"

B="$(app_post "/windows?action=new" | jq -r '.window')"
check "a new empty window opened" test -n "$B"
app_post "/windows?action=sidebar&window=$B&open=true" >/dev/null
app_post "/windows?action=focus&window=$B" >/dev/null
wait_st '.palette.open == true' 5
eq "the empty window opens the palette in place by itself" "$(st '.palette.open')" "true"
check "the app log says why" grep -q "window ${B:0:8} holds no spaces; command palette opened in place" "$MLOG"
sleep 0.5
eq "the empty window draws no sidebar" "$(app_get /geometry | jq -r --arg w "$B" '.windows[$w].sidebar // "none"')" "none"
if [ -n "${WHEEL_SHOTS:-}" ]; then mkdir -p "$WHEEL_SHOTS"; app_post "/window/snapshot?window=$B&frame=1&path=$WHEEL_SHOTS/empty-window-${WHEEL_APPEARANCE:-light}.png" >/dev/null && echo "     shot $WHEEL_SHOTS/empty-window-${WHEEL_APPEARANCE:-light}.png"; fi

app_post "/palette?action=type&text=proj1" >/dev/null
ROW="$(st --arg s "$S1" '[.palette.rows[] | .id] | index("space:" + $s)')"
check "the palette lists the space from the other window" test "$ROW" != "null"
app_post "/palette?action=select&index=$ROW" >/dev/null
app_post "/palette?action=run" >/dev/null
wait_st ".windows[] | select(.id == \"$B\") | .spaces | index(\"$S1\") != null" 5
eq "the picked space comes to the empty window" "$(st -c --arg w "$B" '.windows[] | select(.id == $w) | .spaces')" "[\"$S1\"]"
eq "and leaves the window it was in" "$(st --arg w "$A" --arg s "$S1" '.windows[] | select(.id == $w) | .spaces | index($s)')" "null"
check "the app log says it was brought" grep -q "command palette: space $S1 brought to the empty window ${B:0:8}" "$MLOG"

finish
