#!/usr/bin/env bash
# § 4 Popped windows as drop targets (wheel-f6y5.3): a grid pane's header drag over a popped
# window docks the pane beside the popped pane under the pointer, never the grid pane beneath the
# window; a pane dragged in from another space does the same. Each hover runs the app's own hit
# test (`hover grid=x,y` in the space's grid, `hover at=x,y` on screen). With WHEEL_SHOTS set it
# saves the mid-drag window.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
IDS=($(mkspaces 2))
S1=${IDS[0]} S2=${IDS[1]}
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 2)' 20
A="$(st '.windows[0].id')"
app_post "/windows?action=frame&window=$A&x=40&y=200&w=1100&h=700" >/dev/null

shot() { # shot <name>
  [ -n "${WHEEL_SHOTS:-}" ] || return 0
  mkdir -p "$WHEEL_SHOTS"
  app_post "/window/snapshot?window=$A&path=$WHEEL_SHOTS/$1-${WHEEL_APPEARANCE:-light}.png" >/dev/null && echo "     shot $WHEEL_SHOTS/$1-${WHEEL_APPEARANCE:-light}.png"
}
pane0() { st --arg s "$1" '.spaces[] | select(.id == $s) | .root.id'; }
grid_of() { st -c --arg s "$1" '[.spaces[] | select(.id == $s) | .root | .. | objects | select(.id? and (.kind? != null)) | .id] | sort'; }
popped_of() { st -c --arg s "$1" '[.spaces[] | select(.id == $s) | .popped[] | .panes | sort]'; }

# S1: P1 | P2 in the grid, P3 popped out and placed over P2's half of the grid.
P1="$(pane0 "$S1")"
P2="$(w pane split "$P1" right)"
P3="$(w pane split "$P2" right)"
w pane pop "$P3" >/dev/null
app_post "/focus?space=$S1" >/dev/null
wait_st "[.spaces[].popped[].panes[]] | index(\"$P3\") != null" 10
app_post "/popped/frame?pane=$P3&x=560&y=120&w=420&h=320" >/dev/null
sleep 1
eq "S1 starts with P1 and P2 in the grid and P3 popped" "$(popped_of "$S1")" "[[\"$P3\"]]"

# 1. Over the popped window, which lies over P2: the target is P3, on a side, never the middle.
app_post "/pane/drag?action=start&pane=$P1" >/dev/null
app_post "/pane/drag?action=hover&grid=575,280" >/dev/null
eq "near the popped window's left edge the target is P3's left side" "$(st -c '.pane_drag | [.target, .zone]')" "[\"$P3\",\"left\"]"
app_post "/pane/drag?action=hover&grid=770,281" >/dev/null
eq "in its middle the target is still P3 and still a side" "$(st -c '.pane_drag | [.target, (.zone != "center")]')" "[\"$P3\",true]"
app_post "/pane/drag?action=hover&grid=770,140" >/dev/null
eq "near its top the target is P3's top" "$(st -c '.pane_drag | [.target, .zone]')" "[\"$P3\",\"up\"]"
sleep 0.6; shot popped-drop-over-window
# 2. Off the window, over P2 in the grid: the grid's own drop.
app_post "/pane/drag?action=hover&grid=800,560" >/dev/null
eq "beside the window, over P2, the target is P2" "$(st -r '.pane_drag.target')" "$P2"
# 3. The middle of a pane in another tree docks on its right.
app_post "/pane/drag?action=hover&target=$P3&zone=center" >/dev/null
eq "a named middle drop on a popped pane becomes its right side" "$(st -c '.pane_drag | [.target, .zone]')" "[\"$P3\",\"right\"]"
app_post "/pane/drag?action=hover&grid=575,280" >/dev/null
app_post "/pane/drag?action=drop" >/dev/null
check "the drop lands P1 in P3's window" wait_st "[.spaces[].popped[] | .panes | sort] == [[\"$P1\", \"$P3\"] | sort]" 5
eq "and P2 is the whole grid" "$(st -r --arg s "$S1" '.spaces[] | select(.id == $s) | .root.id')" "$P2"
eq "the drag is over" "$(st '.pane_drag')" "null"
has_log "pane moved" "wheeld logs the move"
shot popped-drop-landed

# 4. From another space: Q2 in S2, dropped on the screen point over S1's popped window.
Q2="$(pane0 "$S2")"
Q2B="$(w pane split "$Q2" right)"
GRID="$(app_get /geometry | jq -c --arg w "$A" '.windows[$w].grid')"
POPPED_FRAME="$(st -c --arg s "$S1" '.spaces[] | select(.id == $s) | .popped[0].frame')"
# A point 12 pt inside the window's left edge, half way down, in screen coordinates (AppKit, y up).
AT="$(jq -nr --argjson g "$GRID" --argjson f "$POPPED_FRAME" '"\($g[0] + $f[0] + 12),\($g[1] + $g[3] - ($f[1] + $f[3] / 2))"')"
app_post "/pane/drag?action=start&pane=$Q2B" >/dev/null
app_post "/pane/drag?action=hover&at=$AT" >/dev/null
eq "from another space, over the popped window, the dock is beside a popped pane" \
  "$(st -c '.pane_drag.away | [.kind, .space]')" "[\"beside\",\"$S1\"]"
TARGET="$(st -r '.pane_drag.away.pane')"
check "and that pane is in the popped window" test "$TARGET" = "$P1" -o "$TARGET" = "$P3"
app_post "/pane/drag?action=drop" >/dev/null
check "Q2B lands in S1's popped window" wait_st "[.spaces[] | select(.id == \"$S1\") | .popped[].panes[]] | index(\"$Q2B\") != null" 5
eq "S2 keeps Q2 in its grid" "$(st -r --arg s "$S2" '.spaces[] | select(.id == $s) | .root.id')" "$Q2"

finish
