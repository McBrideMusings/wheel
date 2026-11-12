#!/usr/bin/env bash
# § Work groups (wheel-h3z5.15): consecutive calls of one kind fold into one row inside a turn's work
# section, and the work line's words follow chat.work.summary (time | counts | both). The fake
# agent's "show tools" turn has two searches, two edits and two commands in a row. /state lists the
# work item with its label and its folds. Drives the real Mac app against this script's own daemon.
# Window shots (cacheDisplay) go to $WHEEL_SHOTS (default $SCR/shots), light and dark.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"

export CLAUDE_CONFIG_DIR="$SCR/claude"
use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset

mkdir -p "$SCR/a"
A="$(w space new "$SCR/a")"
P="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
w pane start "$P" agent >/dev/null

launch_app
check "the app connects" wait_st '.connection == "connected"' 20
app_post "/focus?space=$A" >/dev/null

w agent send "$P" "show tools" >/dev/null
w agent wait "$P" >/dev/null
work() { app_get /state | jq -r --arg p "$P" '.panes[$p].transcript_items[] | select(startswith("work "))'; }
check "the work section reached the app" wait_st ".panes[\"$P\"].transcript_items | map(select(startswith(\"work \"))) | length == 1" 10
echo "--- work:"; work

has() { work | grep -F -- "$1" >/dev/null; }
check "two searches fold" has 'Searched 2 @'
check "two edits fold" has 'Edited files 2 @'
check "two commands fold" has 'Ran commands 2 @'
check "default setting 'time' says Worked for" has 'label "Worked for '

w config set chat.work.summary counts --machine >/dev/null
check "counts: the label names what the calls did" wait_st ".panes[\"$P\"].transcript_items | map(select(startswith(\"work \") and contains(\"label \\\"Read 1 file, searched 2 times, edited 2 files, ran 2 commands\"))) | length == 1" 10
w config set chat.work.summary both --machine >/dev/null
check "both: time then counts" wait_st ".panes[\"$P\"].transcript_items | map(select(startswith(\"work \") and contains(\"label \\\"Worked for \") and contains(\" · Read 1 file, searched\"))) | length == 1" 10

S="$(w space ls | jq -r --arg p "$P" '[.. | objects | select(.id? == $p)][0] | .agent')"
FIRST="$(work | sed 's/^work [0-9]* [^@]*@\(-\{0,1\}[0-9]*\).*/\1/')"
FOLD="$(work | sed -n 's/.*Searched 2 @\(-\{0,1\}[0-9]*\).*/\1/p')"
app_post "/agent/expand?agent=$S&id=$FIRST" >/dev/null || echo "expand refused"
app_post "/agent/expand?agent=$S&id=$FOLD" >/dev/null || echo "expand refused"
check "opening a fold marks it open" wait_st ".panes[\"$P\"].transcript_items | map(select(contains(\"Searched 2 @$FOLD open\"))) | length == 1" 5
for look in light dark; do
  WHEEL_APPEARANCE=$look
  quit_app
  launch_app
  app_post "/focus?space=$A" >/dev/null
  wait_st ".panes[\"$P\"].transcript_tools | length == 11" 20
  app_post "/agent/expand?agent=$S&id=$FIRST" >/dev/null || echo "expand refused"
  app_post "/agent/expand?agent=$S&id=$FOLD" >/dev/null || echo "expand refused"
  sleep 1
  app_post "/window/snapshot?path=$SHOTS/work-groups-$look.png" >/dev/null && echo "shot $SHOTS/work-groups-$look.png"
done
finish
