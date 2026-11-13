#!/usr/bin/env bash
# § 4 Dragging a pane into another space (wheel-i7ve.5): hold over a tab to open its space, dock
# beside one of its panes, drop on a tab, a strip or the desktop, and move it from the pane menu.
# Each drag runs the app's own hit test against the windows' real positions (`hover at=x,y` with a
# point taken from `GET /geometry`), then drops. With WHEEL_SHOTS set it saves the mid-drag window.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
IDS=($(mkspaces 3))
S1=${IDS[0]} S2=${IDS[1]} S3=${IDS[2]}
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 3)' 20
A="$(st '.windows[0].id')"
app_post "/windows?action=frame&window=$A&x=40&y=200&w=1100&h=700" >/dev/null
sleep 1

geo() { app_get /geometry; }
shot() { # shot <name>
  [ -n "${WHEEL_SHOTS:-}" ] || return 0
  mkdir -p "$WHEEL_SHOTS"
  app_post "/window/snapshot?window=$A&path=$WHEEL_SHOTS/$1-${WHEEL_APPEARANCE:-light}.png" >/dev/null && echo "     shot $WHEEL_SHOTS/$1-${WHEEL_APPEARANCE:-light}.png"
}
# The centre of a space's row or tab in window A, as "x,y".
row_of() { geo | jq -r --arg w "$A" --arg s "$1" '.windows[$w].rows[$s] | "\(.[0] + .[2] / 2),\(.[1] + .[3] / 2)"'; }
# A point in the grid's upper-left corner (a pane's left edge), as "x,y".
grid_left() { geo | jq -r --arg w "$A" '.windows[$w].grid | "\(.[0] + 20),\(.[1] + .[3] / 2)"'; }
panes_of() { w space ls --json | jq -r --arg s "$1" '.[] | select(.id == $s) | .panes | sort | join(",")'; }
space_of() { w space ls --json | jq -r --arg p "$1" '.[] | select(.panes | index($p)) | .id'; }
active_of() { st --arg w "$A" '.windows[] | select(.id == $w) | .active'; }
pane0() { st --arg s "$1" '.spaces[] | select(.id == $s) | .root.id'; }

# S1: a terminal that counts, beside a second terminal. S2 and S3: one terminal each.
P1="$(pane0 "$S1")"
w pane kind "$P1" terminal >/dev/null
P2="$(w pane split "$P1" right)"
w pane kind "$P2" terminal >/dev/null
Q2="$(pane0 "$S2")"; w pane kind "$Q2" terminal >/dev/null
Q3="$(pane0 "$S3")"; w pane kind "$Q3" terminal >/dev/null
TERM1="$(st --arg p "$P1" '.panes[$p].term')"
sleep 2
w term send "$TERM1" 'i=0; while true; do echo count$i; i=$((i+1)); sleep 0.2; done' --enter >/dev/null
app_post "/focus?space=$S1" >/dev/null
sleep 1.5
count_of() { w term read "$TERM1" --timeout 0.3 2>/dev/null | grep -o 'count[0-9]*' | tail -1 | tr -d 'a-z'; }
C0="$(count_of)"

# 0. Dragging inside the space: the chip follows the pointer, the pane stays in its slot.
ROOT0="$(st -c --arg s "$S1" '.spaces[] | select(.id == $s) | .root')"
check "no chip before a drag" test "$(st '.pane_drag.chip')" = null
app_post "/pane/drag?action=start&pane=$P1" >/dev/null
check "a started drag shows the chip" test "$(st '.pane_drag.chip')" != null
app_post "/pane/drag?action=hover&target=$P2&zone=right" >/dev/null
eq "over another pane of its space the target is that pane's right edge" "$(st -c '.pane_drag | [.target, .zone]')" "[\"$P2\",\"right\"]"
check "the chip is still up over a pane" test "$(st '.pane_drag.chip')" != null
eq "the daemon's tree is untouched mid-drag" "$(st -c --arg s "$S1" '.spaces[] | select(.id == $s) | .root')" "$ROOT0"
shot pane-drag-in-space
app_post "/pane/drag?action=cancel" >/dev/null
check "cancelling takes the chip down" test "$(st '.pane_drag')" = null
eq "and changes nothing" "$(st -c --arg s "$S1" '.spaces[] | select(.id == $s) | .root')" "$ROOT0"

# 1. Hold over S2's row: the space shows after the wait, and the pane is dropped beside Q2.
app_post "/pane/drag?action=start&pane=$P1" >/dev/null
app_post "/pane/drag?action=hover&at=$(row_of "$S2")" >/dev/null
eq "over a row the drag's target is the end of that space" "$(st -c '.pane_drag.away')" "{\"kind\":\"end\",\"space\":\"$S2\"}"
eq "and the row is the one being waited on" "$(st '.pane_drag.held')" "$S2"
eq "the hint says the pane goes to the end of that space" "$(st '.pane_drag.hint')" "Move “${P1}” to the end of “$(st --arg s "$S2" '.spaces[] | select(.id == $s) | .name')”"
eq "before the wait the window still shows the first space" "$(active_of)" "$S1"
shot pane-drag-tab
check "after about half a second the window shows the held space" wait_st ".windows[0].active == \"$S2\"" 3
app_post "/pane/drag?action=hover&target=$Q2&zone=left" >/dev/null
eq "over a pane of the shown space the target is that pane's left half" "$(st -c '.pane_drag.away | [.kind, .pane, .dir]')" "[\"beside\",\"$Q2\",\"left\"]"
shot pane-drag-other-space
app_post "/pane/drag?action=drop" >/dev/null
sleep 1
eq "the pane is in the other space beside its pane" "$(panes_of "$S2")" "$(printf '%s\n' "$P1" "$Q2" | sort | paste -sd, -)"
eq "its id and terminal are unchanged" "$(st --arg p "$P1" '.panes[$p].term')" "$TERM1"
eq "the source space keeps its other pane" "$(panes_of "$S1")" "$P2"
C1="$(count_of)"
check "the counter kept counting through the move ($C0 then $C1)" test "${C1:-0}" -gt "${C0:-0}"
eq "the drag is over" "$(st '.pane_drag')" "null"
eq "the window still shows the space the pane landed in" "$(active_of)" "$S2"

# 2. A cancelled hold puts back what the window showed.
app_post "/focus?space=$S1" >/dev/null
sleep 0.5
app_post "/pane/drag?action=start&pane=$P2" >/dev/null
app_post "/pane/drag?action=hover&at=$(row_of "$S3")" >/dev/null
wait_st ".windows[0].active == \"$S3\"" 3
app_post "/pane/drag?action=cancel" >/dev/null
sleep 0.4
eq "cancelling shows the space the window showed before" "$(active_of)" "$S1"
eq "and nothing moved" "$(panes_of "$S1")" "$P2"

# 3. Drop on a row: the end of that space, the window keeps showing its own.
app_post "/pane/drag?action=start&pane=$P2" >/dev/null
app_post "/pane/drag?action=hover&space=$S3" >/dev/null
sleep 0.1
app_post "/pane/drag?action=drop" >/dev/null
sleep 1
eq "dropping on a row puts the pane at the end of that space" "$(space_of "$P2")" "$S3"
eq "the emptied source space holds one new empty pane" "$(w space ls --json | jq -r --arg s "$S1" '.[] | select(.id == $s) | .panes | length')" "1"

# 3b. Inside its own space a drag still swaps: no away target, the tree changes on the drop.
FIRST="$(st --arg s "$S3" '.spaces[] | select(.id == $s) | .root.a.id')"
SECOND="$(st --arg s "$S3" '.spaces[] | select(.id == $s) | .root.b.id')"
app_post "/pane/drag?action=start&pane=$FIRST" >/dev/null
app_post "/pane/drag?action=hover&target=$SECOND&zone=center" >/dev/null
eq "over a pane of its own space the drag has a drop and no away target" "$(st -c '[.pane_drag.target, .pane_drag.away]')" "[\"$SECOND\",null]"
app_post "/pane/drag?action=drop" >/dev/null
sleep 1
eq "the drop swaps the two panes" "$(st --arg s "$S3" '.spaces[] | select(.id == $s) | .root.a.id')" "$SECOND"

# 4. The pane menu.
eq "the pane menu lists a Move Pane to Space item per space, the pane's own disabled, and both new-space items" "$(st --arg p "$P1" --arg s "$S2" '.panes[$p].menu | ((map(select(.id == "move-space:" + $s and .enabled == false)) | length) == 1) and ((map(select(.id == "move-new-space")) | length) == 1) and ((map(select(.id == "move-new-little")) | length) == 1)')" "true"
eq "the submenu marks the pane's own space here" "$(st --arg p "$P1" --arg s "$S2" '[.pane_move_menu[$p][].rows[] | select(.here) | .space] == [$s]')" "true"
app_post "/pane/menu?pane=$P1&item=move-space:$S3" >/dev/null
sleep 1
eq "Move Pane to Space puts the pane at the end of the chosen space" "$(space_of "$P1")" "$S3"
eq "the pane menu refuses the pane's own space" "$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $(TOKEN)" "http://127.0.0.1:$DEBUG_PORT/pane/menu?pane=$P1&item=move-space:$S3")" "409"

N_BEFORE="$(w space ls --json | jq 'length')"
app_post "/pane/menu?pane=$P1&item=move-new-space" >/dev/null
sleep 1.5
NEW="$(space_of "$P1")"
eq "Move Pane to New Space makes a space holding the pane" "$(panes_of "$NEW")" "$P1"
eq "and one space more exists" "$(w space ls --json | jq 'length')" "$((N_BEFORE + 1))"
eq "the new space is in the pane's window" "$(win_of "$NEW")" "$A"
check "Move Pane to New Space shows the new space in that window" wait_st ".windows[] | select(.id == \"$A\") | .active == \"$NEW\"" 5
eq "right after the space the pane left" "$(st --arg w "$A" --arg n "$NEW" '.windows[] | select(.id == $w) | .spaces | index($n)')" "$(st --arg w "$A" --arg s "$S3" '.windows[] | select(.id == $w) | .spaces | index($s) + 1')"

app_post "/pane/menu?pane=$P1&item=move-new-little" >/dev/null
sleep 2
LITTLE_SPACE="$(space_of "$P1")"
check "Move Pane to New Little Window opens a little window on a new space holding the pane" test "$(st --arg s "$LITTLE_SPACE" '[.little_windows[] | select(.space == $s)] | length')" = 1
eq "owned by the pane's window" "$(st --arg s "$LITTLE_SPACE" '.little_windows[] | select(.space == $s) | .owner')" "$A"
eq "the pane is alone in it" "$(panes_of "$LITTLE_SPACE")" "$P1"

# 5. The empty part of a tab strip or sidebar makes a new space in that window.
app_post "/pane/drag?action=start&pane=$P2" >/dev/null
app_post "/pane/drag?action=hover&newspace=$A" >/dev/null
eq "over the empty strip the target is a new space in the window" "$(st -c '.pane_drag.away')" "{\"kind\":\"new_space\",\"window\":\"$A\"}"
app_post "/pane/drag?action=drop" >/dev/null
sleep 1.5
eq "the dropped pane is alone in a space of its own" "$(panes_of "$(space_of "$P2")")" "$P2"
eq "in the window dropped on" "$(win_of "$(space_of "$P2")")" "$A"

# 6. The desktop: a point where no Wheel window is makes a little window.
app_post "/pane/drag?action=start&pane=$P2" >/dev/null
app_post "/pane/drag?action=hover&at=1500,20" >/dev/null
eq "over the empty desktop the target is the desktop" "$(st '.pane_drag.away.kind')" "desktop"
eq "and the chip names the pane and the little window" "$(st '.pane_drag.chip | test("new little window")')" "true"
app_post "/pane/drag?action=drop" >/dev/null
sleep 2
DESK="$(space_of "$P2")"
check "dropping on the desktop opens a little window holding the pane" test "$(st --arg s "$DESK" '[.little_windows[] | select(.space == $s)] | length')" = 1
eq "owned by the window the pane came from" "$(st --arg s "$DESK" '.little_windows[] | select(.space == $s) | .owner')" "$A"
finish
