#!/usr/bin/env bash
# § 7 Status dots: the Mac app draws the status the daemon sends on pane headers, sidebar rows and
# tabs, and `status.dots` hides ranks. Drives the real Mac app through its debug server against
# this script's own daemon. Window shots (cacheDisplay, never screencapture) go to $WHEEL_SHOTS
# (default: $SCR/shots), one per theme and sidebar state.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset

# Space A holds two agent panes (one waiting on a permission, one with a turn running), B an idle
# agent, C an agent that could not start, D a pane that runs no agent.
mkdir -p "$SCR/a" "$SCR/b" "$SCR/c" "$SCR/d"
A="$(w space new "$SCR/a")"; B="$(w space new "$SCR/b")"; C="$(w space new "$SCR/c")"; D="$(w space new "$SCR/d")"
pane_of() { w space ls | jq -r --arg s "$1" '.spaces[] | select(.id==$s) | .focus'; }
PA1="$(pane_of "$A")"
w pane split "$PA1" right >/dev/null
PA2="$(w space ls | jq -r --arg s "$A" --arg p "$PA1" '[.spaces[] | select(.id==$s) | .root | .. | objects | select(.node=="pane") | .id | select(. != $p)] | first')"
PB="$(pane_of "$B")"; PC="$(pane_of "$C")"; PD="$(pane_of "$D")"
rmdir "$SCR/c"
w pane start "$PA1" agent >/dev/null
w pane start "$PA2" agent >/dev/null
w pane start "$PB" agent --message "hello" >/dev/null
w pane start "$PC" agent >/dev/null
w pane start "$PD" terminal >/dev/null
bg w agent send "$PA1" "permission please"
bg w agent send "$PA2" "work slowly 600"

launch_app
check "the app connects" wait_st '.connection == "connected"' 20
wait_pane() { wait_st ".panes[\"$1\"].status.kind == \"$2\"" "${3:-20}"; }
check "the waiting pane is needs_you with 1 waiting" wait_st ".panes[\"$PA1\"].status | .kind == \"needs_you\" and .waiting == 1" 20
eq "the pane header's label" "$(st --arg p "$PA1" '.panes[$p].status.label')" "Needs you, 1 waiting"
check "the running pane is working" wait_pane "$PA2" working
check "the idle pane is idle" wait_pane "$PB" idle
check "the pane that could not start is stopped" wait_pane "$PC" stopped 90
eq "a terminal pane has no status" "$(st --arg p "$PD" '.panes[$p].status')" "null"
eq "a space with no agent pane has no status" "$(st --arg s "$D" '.spaces[] | select(.id==$s) | .status')" "null"
eq "space A takes the highest rank" "$(st --arg s "$A" '.spaces[] | select(.id==$s) | .status.kind')" "needs_you"
eq "its sidebar row carries the caption" "$(st --arg s "$A" '.sidebar.rows[] | select(.space==$s) | .caption')" "1 waiting"
eq "space C's row says stopped" "$(st --arg s "$C" '.sidebar.rows[] | select(.space==$s) | .caption')" "stopped"
eq "all four ranks draw by default" "$(st -c '.status_dots')" '["needs_you","stopped","working","idle"]'

shoot() { # shoot <name>: both themes of the current window
  local theme
  for theme in light dark; do
    WHEEL_APPEARANCE=$theme
    app_post /app/quit >/dev/null 2>&1; poll 15 sh -c "! kill -0 $(app_pid) 2>/dev/null" || true
    launch_app
    wait_st '.connection == "connected"' 20
    wait_pane "$PC" stopped
    app_post "/focus?space=$A" >/dev/null
    app_post "/window/front" >/dev/null
    sleep 1
    app_post "/window/snapshot?path=$SHOTS/$1-$theme.png" >/dev/null
    echo "shot $SHOTS/$1-$theme.png"
  done
  unset WHEEL_APPEARANCE
}

app_post "/sidebar?open=true" >/dev/null
shoot sidebar-open
app_post "/sidebar?open=false" >/dev/null
shoot sidebar-closed
eq "closed: the tabs list the spaces with their status" "$(st --arg s "$A" '.sidebar.tabs[] | select(.space==$s) | .status.kind')" "needs_you"

# Answering the request brings the pane back to working, the space to working or higher.
PERM="$(perm_id)"
w perm answer "$PERM" allow >/dev/null
check "answering the request brings the pane back to working" wait_pane "$PA1" working

# status.dots hides ranks.
w config set status.dots needs_you,stopped --machine >/dev/null
check "status.dots reaches the app" wait_st '.status_dots == ["needs_you","stopped"]' 10
eq "an idle pane keeps its status but is not drawn" "$(st --arg p "$PB" '.panes[$p].status | "\(.kind) \(.drawn)"')" "idle false"
eq "a working pane is not drawn" "$(st --arg p "$PA2" '.panes[$p].status | "\(.kind) \(.drawn)"')" "working false"
eq "a stopped pane is drawn" "$(st --arg p "$PC" '.panes[$p].status | "\(.kind) \(.drawn)"')" "stopped true"
w config set status.dots "" --machine >/dev/null
check "an empty status.dots draws nothing" wait_st '.status_dots == []' 10
app_post "/window/front" >/dev/null
sleep 1
app_post "/window/snapshot?path=$SHOTS/none-drawn.png" >/dev/null
w config unset status.dots --machine >/dev/null
check "unsetting it draws every rank again" wait_st '.status_dots | length == 4' 10

check "the app log records the setting" grep -q "status.dots = needs_you,stopped" "$(st .log_path)"
finish
