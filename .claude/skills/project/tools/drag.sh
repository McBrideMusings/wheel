#!/usr/bin/env bash
# § 4 Dragging spaces: reorder in a sidebar, between windows, onto the desktop, from the title
# chip, and in the menu bar popover. Each drag runs the app's own hit test against the windows'
# real positions (`hover at=x,y` with a point taken from `GET /geometry`), then drops.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
IDS=($(mkspaces 6))
S1=${IDS[0]} S2=${IDS[1]} S3=${IDS[2]} S4=${IDS[3]} S5=${IDS[4]} S6=${IDS[5]}
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 6)' 20
A="$(st '.windows[0].id')"
app_post "/windows?action=place&space=$S4&window=$A" >/dev/null   # order stays s1..s6; s4 shown
app_post "/windows?action=new&spaces=$S5,$S6" >/dev/null
wait_st '.windows | length == 2'
B="$(win_of "$S5")"
# Side by side, so neither covers the other.
app_post "/windows?action=frame&window=$A&x=40&y=200&w=900&h=600" >/dev/null
app_post "/windows?action=frame&window=$B&x=1000&y=200&w=900&h=600" >/dev/null
sleep 1

geo() { app_get /geometry; }
# A point in the upper half of a sidebar row (the gap opens above that row), as "x,y".
upper_of() { geo | jq -r --arg w "$1" --arg s "$2" '.windows[$w].rows[$s] | "\(.[0] + 20),\(.[1] + .[3] * 0.8)"'; }
lower_end_of() { geo | jq -r --arg w "$1" '.windows[$w].sidebar | "\(.[0] + 20),\(.[1] + 8)"'; }
order() { st --arg w "$1" '.windows[] | select(.id==$w) | .spaces | join(",")'; }
drag() { # drag <space> <x,y> : start, hover at the point, drop
  app_post "/spacedrag?action=start&space=$1" >/dev/null
  app_post "/spacedrag?action=hover&at=$2" >/dev/null
  app_post "/spacedrag?action=drop" >/dev/null
  sleep 0.4
}


# 1. Reorder: the third space above the first.
ROWS_BEFORE="$(geo | jq -c --arg w "$A" '.windows[$w].rows')"
app_post "/spacedrag?action=start&space=$S3" >/dev/null
app_post "/spacedrag?action=hover&at=$(upper_of "$A" "$S1")" >/dev/null
sleep 0.5
eq "mid-drag no sidebar row has moved" "$(geo | jq -c --arg w "$A" '.windows[$w].rows')" "$ROWS_BEFORE"
eq "a chip follows the pointer, named for the space" "$(st '.space_drag.chip')" "$(basename "$SCR/proj3")"
if [ -n "${WHEEL_SHOTS:-}" ]; then mkdir -p "$WHEEL_SHOTS"; app_post "/window/snapshot?window=$A&path=$WHEEL_SHOTS/drag-mid-${WHEEL_APPEARANCE:-light}.png" >/dev/null && echo "     shot $WHEEL_SHOTS/drag-mid-${WHEEL_APPEARANCE:-light}.png"; fi
check "dragging over a sidebar says where it will land" test "$(st '.space_drag.hint')" = "Place “$(basename "$SCR/proj3")” here in “$(st --arg w "$A" '.windows[] | select(.id==$w) | .title')”"
app_post "/spacedrag?action=drop" >/dev/null
sleep 0.4
eq "the third space is above the first" "$(order "$A" | cut -d, -f1-3)" "$S3,$S1,$S2"
eq "the drag is over" "$(st '.space_drag')" "null"
quit_app; launch_app
wait_st '.windows | length == 2'
eq "the order is the same after relaunch" "$(order "$A" | cut -d, -f1-3)" "$S3,$S1,$S2"
app_post "/windows?action=frame&window=$A&x=40&y=200&w=900&h=600" >/dev/null
app_post "/windows?action=frame&window=$B&x=1000&y=200&w=900&h=600" >/dev/null
sleep 1

# 2. Between windows: s2 from A's sidebar into B's, before its first row.
drag "$S2" "$(upper_of "$B" "$S5")"
eq "the space is in B only, at the dropped index" "$(order "$B" | cut -d, -f1)" "$S2"
eq "and A no longer lists it" "$(st --arg s "$S2" --arg w "$A" '.windows[] | select(.id==$w) | .spaces | index($s)')" "null"

# 3. The desktop: a point where no Wheel window is.
app_post "/spacedrag?action=start&space=$S1" >/dev/null
app_post "/spacedrag?action=hover&at=960,20" >/dev/null
check "over empty desktop the preview card is shown" wait_st '.space_drag.preview_card' 3
eq "and the hint says it opens a little window" "$(st '.space_drag.hint')" "Release to open “proj1” in a little window"
app_post "/spacedrag?action=drop" >/dev/null
sleep 0.5
eq "the space is in a little window owned by its window" "$(st --arg s "$S1" '[.little_windows[] | select(.space == $s) | .owner] | join(" ")')" "$A"
eq "no third window opened" "$(st '.windows | length')" "2"
eq "the preview card is gone after the drop" "$(st '.space_drag')" "null"
eq "the source window still lists the space" "$(st --arg s "$S1" --arg w "$A" '.windows[] | select(.id==$w) | .spaces | index($s) != null')" "true"
app_post "/windows?action=littleback&space=$S1" >/dev/null

# Escape cancels: nothing moves.
BEFORE="$(st -c '[.windows[] | .spaces]')"
app_post "/spacedrag?action=start&space=$S2" >/dev/null
app_post "/spacedrag?action=hover&at=960,20" >/dev/null
app_post "/spacedrag?action=cancel" >/dev/null
eq "a cancelled drag moves nothing and closes the outline" "$(st -c '[.windows[] | .spaces]'):$(st '.space_drag')" "$BEFORE:null"

# 4. The title chip of a window with its sidebar hidden drags the active space onto another window.
app_post "/windows?action=sidebar&window=$A&open=false" >/dev/null
ACTIVE="$(st --arg w "$A" '.windows[] | select(.id==$w) | .active')"
app_post "/spacedrag?action=start&space=$ACTIVE" >/dev/null
app_post "/spacedrag?action=hover&target=window:$B" >/dev/null
eq "over another window's body the hint says move" "$(st '.space_drag.hint' | cut -c1-8)" "Move “pr"
app_post "/spacedrag?action=drop" >/dev/null
sleep 0.4
eq "the chip's space moved to the end of the other window" "$(order "$B" | awk -F, '{print $NF}')" "$ACTIVE"

# 4b. The tab strip of a window with its sidebar hidden: an insertion bar between tabs, none beside the dragged tab.
TABS=($(order "$A" | tr ',' ' '))
T_FIRST=${TABS[0]} T_LAST=${TABS[${#TABS[@]}-1]}
# A point inside a tab's left or right edge, as "x,y".
tab_left() { geo | jq -r --arg w "$A" --arg s "$1" '.windows[$w].rows[$s] | "\(.[0] + 6),\(.[1] + .[3] / 2)"'; }
tab_right() { geo | jq -r --arg w "$A" --arg s "$1" '.windows[$w].rows[$s] | "\(.[0] + .[2] - 6),\(.[1] + .[3] / 2)"'; }
check "the tab strip is measured" test "$(geo | jq -r --arg w "$A" '.windows[$w].tab_strip | length')" = 4
app_post "/spacedrag?action=start&space=$T_LAST" >/dev/null
app_post "/spacedrag?action=hover&at=$(tab_left "$T_FIRST")" >/dev/null
eq "over a tab's left edge the bar is left of that tab" "$(st -c '.space_drag.insertion')" "{\"index\":0,\"window\":\"$A\"}"
eq "and the hint says where it lands" "$(st '.space_drag.hint' | cut -c1-8)" "Place “p"
if [ -n "${WHEEL_SHOTS:-}" ]; then mkdir -p "$WHEEL_SHOTS"; app_post "/window/snapshot?window=$A&path=$WHEEL_SHOTS/drag-tabs-${WHEEL_APPEARANCE:-light}.png" >/dev/null && echo "     shot $WHEEL_SHOTS/drag-tabs-${WHEEL_APPEARANCE:-light}.png"; fi
app_post "/spacedrag?action=drop" >/dev/null
sleep 0.4
eq "dropping there puts the space first among the tabs" "$(order "$A" | cut -d, -f1)" "$T_LAST"
# The dragged tab is now first: the gaps on either side of it change nothing, so no bar.
app_post "/spacedrag?action=start&space=$T_LAST" >/dev/null
app_post "/spacedrag?action=hover&at=$(tab_left "$T_LAST")" >/dev/null
eq "no bar before the dragged tab" "$(st -c '.space_drag.insertion')" "null"
app_post "/spacedrag?action=hover&at=$(tab_left "$T_FIRST")" >/dev/null
eq "no bar after the dragged tab" "$(st -c '.space_drag.insertion')" "null"
app_post "/spacedrag?action=hover&at=$(tab_right "$(order "$A" | awk -F, '{print $NF}')")" >/dev/null
eq "a bar shows past the last tab" "$(st -c '.space_drag.insertion')" "{\"index\":${#TABS[@]},\"window\":\"$A\"}"
app_post "/spacedrag?action=cancel" >/dev/null

# 5. The popover: a window's space onto Background, and a background space onto a window card.
open_popover() { app_post "/menubar?action=open" >/dev/null; wait_st '.menu_bar.open' 5; popover_ready; }
popover_ready() { for _ in $(seq 1 100); do [ "$(geo | jq -r '(.popover.rows // {} | length) > 0')" = true ] && return 0; sleep 0.1; done; return 1; }
app_post "/menubar?action=open" >/dev/null
wait_st '.menu_bar.open' 5
check "the popover's cards and rows are measured" popover_ready
card_of() { geo | jq -r --arg k "$1" '.popover.cards[$k] | "\(.[0] + .[2] / 2),\(.[1] + .[3] / 2)"'; }
VICTIM="$(st --arg w "$B" '.windows[] | select(.id==$w) | .spaces[0]')"
app_post "/spacedrag?action=start&space=$VICTIM" >/dev/null
app_post "/spacedrag?action=hover&at=$(card_of background)" >/dev/null
eq "over the Background card the hint says it runs with no window" "$(st '.space_drag.hint' | cut -c1-9)" "Run “proj"
app_post "/spacedrag?action=drop" >/dev/null
sleep 0.4
eq "dragging a window's space onto Background backgrounds it" "$(st -c '.background | index("'"$VICTIM"'") != null')" "true"
open_popover
app_post "/spacedrag?action=start&space=$VICTIM" >/dev/null
app_post "/spacedrag?action=hover&at=$(card_of "$A")" >/dev/null
app_post "/spacedrag?action=drop" >/dev/null
sleep 0.4
eq "dragging a background space onto a window card moves it there" "$(win_of "$VICTIM")" "$A"

# 5b. A row dropped inside a popover group lands at the dropped index, with a bar drawn there.
order_a() { order "$A"; }
check "the popover is open again for the group drags" open_popover
FIRST_A="$(order_a | cut -d, -f1)"
LAST_A="$(order_a | awk -F, '{print $NF}')"
app_post "/spacedrag?action=start&space=$LAST_A" >/dev/null
app_post "/spacedrag?action=hover&target=card:$A:0" >/dev/null
eq "over a group's first gap the bar reports popover:<window> at 0" "$(st -c '.space_drag.insertion')" "{\"index\":0,\"window\":\"popover:$A\"}"
if [ -n "${WHEEL_SHOTS:-}" ]; then mkdir -p "$WHEEL_SHOTS"; app_post "/menubar?action=snapshot&path=$WHEEL_SHOTS/drag-popover-${WHEEL_APPEARANCE:-light}.png" >/dev/null && echo "     shot $WHEEL_SHOTS/drag-popover-${WHEEL_APPEARANCE:-light}.png"; fi
app_post "/spacedrag?action=drop" >/dev/null
sleep 0.4
eq "dropping there puts the space first in its window" "$(order_a | cut -d, -f1)" "$LAST_A"
# The drop focused a window, which closed the popover: open it again before aiming at its rows.
check "the reopened popover's rows are measured" open_popover
# A real point inside the group: the gap under the pointer picks the index.
app_post "/spacedrag?action=start&space=$LAST_A" >/dev/null
app_post "/spacedrag?action=hover&at=$(geo | jq -r --arg w "$A" --arg s "$FIRST_A" '.popover.rows[$w][$s] | "\(.[0] + 20),\(.[1] + .[3] * 0.2)"')" >/dev/null
eq "a point in the lower half of a row puts the bar below it" "$(st -c '.space_drag.insertion')" "{\"index\":2,\"window\":\"popover:$A\"}"
app_post "/spacedrag?action=cancel" >/dev/null
# The Background group takes a drop at an index too.
app_post "/spacedrag?action=start&space=$FIRST_A" >/dev/null
app_post "/spacedrag?action=hover&target=card:background:0" >/dev/null
app_post "/spacedrag?action=drop" >/dev/null
sleep 0.4
eq "a drop at Background index 0 lands first in the background" "$(st '.background[0]')" "$FIRST_A"

# 5c. The desktop from the popover: a background row opens an ownerless little window, a window row an owned one, and a space already in one moves it.
app_post "/spacedrag?action=start&space=$FIRST_A" >/dev/null
app_post "/spacedrag?action=hover&at=960,20" >/dev/null
app_post "/spacedrag?action=drop" >/dev/null
sleep 0.6
eq "a background row dropped on the desktop opens a little window with no owner" "$(st --arg s "$FIRST_A" '[.little_windows[] | select(.space == $s) | .owner // "none"] | join(" ")')" "none"
FRAME_BEFORE="$(st -c --arg s "$FIRST_A" '[.little_windows[] | select(.space == $s) | .frame][0]')"
app_post "/spacedrag?action=start&space=$FIRST_A" >/dev/null
app_post "/spacedrag?action=hover&at=300,40" >/dev/null
app_post "/spacedrag?action=drop" >/dev/null
sleep 0.6
eq "dropped on the desktop again it moves that window, no second one" "$(st --arg s "$FIRST_A" '[.little_windows[] | select(.space == $s)] | length')" "1"
FRAME_AFTER="$(st -c --arg s "$FIRST_A" '[.little_windows[] | select(.space == $s) | .frame][0]')"
check "and the window is at the new drop point" test "$FRAME_BEFORE" != "$FRAME_AFTER"
app_post "/windows?action=littleback&space=$FIRST_A" >/dev/null
OWNED_FROM="$(order "$B" | cut -d, -f1)"
app_post "/spacedrag?action=start&space=$OWNED_FROM" >/dev/null
app_post "/spacedrag?action=hover&at=960,20" >/dev/null
app_post "/spacedrag?action=drop" >/dev/null
sleep 0.6
eq "a window group's row dropped on the desktop opens a little window owned by that window" "$(st --arg s "$OWNED_FROM" '[.little_windows[] | select(.space == $s) | .owner] | join(" ")')" "$B"
app_post "/windows?action=littleback&space=$OWNED_FROM" >/dev/null
app_post "/menubar?action=close" >/dev/null

# 6. A terminal in a moved space is redrawn at the new window's grid size.
T="$(st --arg s "$S4" '.spaces[] | select(.id==$s) | .root.id')"
w pane kind "$T" terminal >/dev/null
sleep 2
app_post "/windows?action=frame&window=$A&x=40&y=200&w=900&h=600" >/dev/null
app_post "/windows?action=frame&window=$B&x=1000&y=200&w=1300&h=800" >/dev/null
app_post "/windows?action=place&space=$S4&window=$A" >/dev/null
sleep 1.5
SMALL="$(st --arg p "$T" '.panes[$p] | "\(.cols)x\(.rows)"')"
app_post "/windows?action=place&space=$S4&window=$B" >/dev/null
sleep 2
BIG="$(st --arg p "$T" '.panes[$p] | "\(.cols)x\(.rows)"')"
check "the pane is laid out at a different size in the larger window" test "$SMALL" != "$BIG"
TERM_ID="$(st --arg p "$T" '.panes[$p].term')"
eq "the daemon's size for the terminal matches the pane" "$(w status | jq -r --arg t "$TERM_ID" '.terms[] | select(.id==$t) | "\(.cols)x\(.rows)"')" "$BIG"
finish
