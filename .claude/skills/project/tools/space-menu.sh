#!/usr/bin/env bash
# § 4 A space's context menu (its tab, sidebar row and title chip) and `wheel space focus` on a
# background space. Every item runs through `POST /space/menu`, the call the menu's buttons make,
# and is read back from `/state` and the CLI.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
IDS=($(mkspaces 4))
S1=${IDS[0]} S2=${IDS[1]} S3=${IDS[2]} S4=${IDS[3]}
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 4)' 20
A="$(st '.windows[0].id')"
APPLOG="$(st .log_path)"
order() { st --arg w "$1" '.windows[] | select(.id==$w) | .spaces | join(",")'; }
menu() { app_post "/space/menu?space=$1&item=$2" >/dev/null; }
item() { st --arg s "$1" --arg i "$2" ".space_menus[\$s][] | select(.id==\$i) | $3"; }

# What the menu lists, in order, and what is off.
eq "the menu lists its items in order" "$(st --arg s "$S1" '[.space_menus[$s][].id] | join(",")')" \
  "new-window,background,left,right,rename,copy-path,reveal,new-pane,close-others,close"
eq "Move Left is off on the first space" "$(item "$S1" left .enabled)" "false"
eq "Move Right is off on the last space" "$(item "$S4" right .enabled)" "false"
eq "both close items are destructive" "$(st --arg s "$S1" '[.space_menus[$s][] | select(.destructive) | .id] | join(",")')" "close-others,close"

# Reorder without a mouse; the window keeps showing what it showed.
app_post "/windows?action=place&space=$S4&window=$A&at=3" >/dev/null   # order stays s1..s4; s4 shown
SHOWN="$(st --arg w "$A" '.windows[] | select(.id==$w) | .active')"
menu "$S1" right
eq "Move Right swaps it with the next space" "$(order "$A")" "$S2,$S1,$S3,$S4"
eq "a reorder leaves the window showing the same space" "$(st --arg w "$A" '.windows[] | select(.id==$w) | .active')" "$SHOWN"
menu "$S1" left
eq "Move Left puts it back" "$(order "$A")" "$S1,$S2,$S3,$S4"
expect_exit 22 "Move Left on the first space is refused" app_post "/space/menu?space=$S1&item=left"

# Copy Path puts the space's directory on the clipboard.
menu "$S2" copy-path
eq "Copy Path copies the space's directory" "$(st .pairing.clipboard.string)" "$(w space ls | jq -r --arg s "$S2" '.spaces[] | select(.id==$s) | .cwd')"

# New Pane adds one empty pane to the space.
before="$(w space ls | jq --arg s "$S2" '[.spaces[] | select(.id==$s) | .. | objects | select(.node=="pane")] | length')"
menu "$S2" new-pane
sleep 1
eq "New Pane adds one pane" "$(w space ls | jq --arg s "$S2" '[.spaces[] | select(.id==$s) | .. | objects | select(.node=="pane")] | length')" "$((before + 1))"

# Placement: a new window, another window, the background.
menu "$S4" new-window
check "Move to New Window opens a window holding only it" wait_st ".windows | length == 2" 5
B="$(win_of "$S4")"
eq "the new window holds only that space" "$(order "$B")" "$S4"
eq "the first window lists the other window under Move to Window" "$(st --arg s "$S1" --arg b "window:$B" '[.space_menus[$s][] | select(.id==$b)] | length')" "1"
menu "$S3" "window:$B"
eq "Move to Window puts it in that window" "$(order "$B")" "$S4,$S3"
menu "$S3" background
check "Send to Background moves it out of every window" wait_st ".background | index(\"$S3\") != null" 5
eq "the other window keeps its space" "$(order "$B")" "$S4"

# A background space stays there when wheeld makes it active (from another active space, so it is news).
w space focus "$S1" >/dev/null
sleep 1
w space focus "$S3" >/dev/null
sleep 1
eq "space focus on a background space leaves it in the background" "$(st ".background | index(\"$S3\") != null")" "true"
eq "the first window's spaces are unchanged" "$(order "$A")" "$S1,$S2"
check "the app logs that it left the space in the background" grep -q "space $S3 made active by wheeld; left in the background" "$APPLOG"

# Close Other Spaces asks first, then closes the rest of the window.
menu "$S1" close-others
eq "Close Other Spaces asks first" "$(st -r '.confirmation.title // empty')" "Close 1 other space in this window?"
app_post "/confirm?answer=no" >/dev/null
eq "declining closes nothing" "$(order "$A")" "$S1,$S2"
menu "$S1" close-others
app_post "/confirm?answer=yes" >/dev/null
check "accepting closes the other space" wait_st "[.spaces[].id] | index(\"$S2\") == null" 5
eq "the kept space is still in its window" "$(order "$A")" "$S1"
check "the app logs each pick" grep -q "space menu $S1: close-others" "$APPLOG"

finish
