#!/usr/bin/env bash
# § 7 Status dots, many agents: six agent panes in one space draw four marks and "+2" in the sidebar row and the tab (wheel-bjmm). Shots of both, light and dark, go to $WHEEL_SHOTS.
T="$(dirname "${BASH_SOURCE[0]}")"
source "$T/lib.sh"
source "$T/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"; mkdir -p "$SHOTS"
use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
mkdir -p "$SCR/a" "$SCR/b"
A="$(w space new "$SCR/a")"; B="$(w space new "$SCR/b")"
P1="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
panes=("$P1")
last="$P1"
for i in 2 3 4 5 6; do
  n="$(w pane split "$last" right)"; panes+=("$n"); last="$n"
done
PB="$(w space ls | jq -r --arg s "$B" '.spaces[] | select(.id==$s) | .focus')"
w pane start "${panes[0]}" agent >/dev/null
w pane start "${panes[1]}" agent >/dev/null
w pane start "${panes[2]}" agent --message "hello" >/dev/null
w pane start "${panes[3]}" agent --message "hello" >/dev/null
w pane start "${panes[4]}" agent --message "hello" >/dev/null
w pane start "${panes[5]}" agent >/dev/null
w pane start "$PB" agent --message "hello" >/dev/null
bg w agent send "${panes[0]}" "permission please"
bg w agent send "${panes[1]}" "work slowly 600"
bg w agent send "${panes[5]}" "work slowly 600"
launch_app
check "the app connects" wait_st '.connection == "connected"' 20
check "the waiting pane is needs_you" wait_st ".panes[\"${panes[0]}\"].status.kind == \"needs_you\"" 20
check "the last pane is working" wait_st ".panes[\"${panes[5]}\"].status.kind == \"working\"" 20
check "an idle pane is idle" wait_st ".panes[\"${panes[4]}\"].status.kind == \"idle\"" 20
for theme in light dark; do
  WHEEL_APPEARANCE=$theme
  app_post /app/quit >/dev/null 2>&1; poll 15 sh -c "! kill -0 $(app_pid) 2>/dev/null" || true
  launch_app
  wait_st '.connection == "connected"' 20
  app_post "/focus?space=$A" >/dev/null
  app_post "/sidebar?open=true" >/dev/null
  app_post "/window/front" >/dev/null
  sleep 2
  app_post "/window/snapshot?path=$SHOTS/six-open-$theme.png" >/dev/null
  echo "shot $SHOTS/six-open-$theme.png"
  app_post "/sidebar?open=false" >/dev/null
  sleep 1
  app_post "/window/snapshot?path=$SHOTS/six-tabs-$theme.png" >/dev/null
  echo "shot $SHOTS/six-tabs-$theme.png"
done
finish
