#!/usr/bin/env bash
# § The chat's work sections (wheel-h3z5.33): every message the agent writes is drawn in the main
# chat; the tool calls and thoughts between two messages fold into one collapsed section. Drives
# the real Mac app through its debug server against this script's own daemon.
#
# REAL_TRANSCRIPT (default: the 13m 9s Steam Deck conversation behind the report, in
# ~/.claude/projects/-Users-pierce-Upstream-The-Order-1886-pc/) is copied into the fake agent's
# store and resumed, so the app draws the real turns. The script skips that part, and says so,
# when the file is not there. Window shots (cacheDisplay) go to $WHEEL_SHOTS (default $SCR/shots).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"
REAL="${REAL_TRANSCRIPT:-$HOME/.claude/projects/-Users-pierce-Upstream-The-Order-1886-pc/f0854844-425a-4569-b0dd-1d1277d062a6.jsonl}"

export CLAUDE_CONFIG_DIR="$SCR/claude"
use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset

mkdir -p "$SCR/a"
A="$(w space new "$SCR/a")"
P="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
w pane start "$P" agent >/dev/null
S="$(w space ls | jq -r --arg p "$P" '[.. | objects | select(.id? == $p) | .session?] | map(select(. != null)) | first')"

launch_app
check "the app connects" wait_st '.connection == "connected"' 20
app_post "/focus?space=$A" >/dev/null

# A turn of three messages with work before, between and after them.
w agent send "$P" "work slowly 1" >/dev/null
w agent send "$P" "reflect on it, then work slowly 1" >/dev/null
w agent wait "$P" >/dev/null
items() { app_get /state | jq -r --arg p "$P" '.panes[$p].transcript_items[]'; }
echo "--- fake agent turns:"; items
check "a message between two stretches of work is a row of its own" \
  wait_st ".panes[\"$P\"].transcript_items | map(select(startswith(\"agent \"))) | length >= 2" 5
check "work before and after a message are separate sections" \
  wait_st ".panes[\"$P\"].transcript_items | map(select(startswith(\"work \"))) | length >= 2" 5

check "every section starts collapsed" \
  wait_st ".panes[\"$P\"].transcript_items | map(select(startswith(\"work \") and contains(\" open\"))) | length == 0" 5
FIRST="$(items | grep '^work ' | head -1 | sed 's/^work [0-9]* @\(-[0-9]*\).*/\1/')"
echo "session=$S pane=$P"
app_post "/agent/expand?session=$S&id=$FIRST" >/dev/null || echo "expand refused"
check "opening one section opens only that one" \
  wait_st ".panes[\"$P\"].transcript_items | map(select(startswith(\"work \") and contains(\"@$FIRST open\"))) | length == 1 and (map(select(startswith(\"work \") and (contains(\" open\")))) | length == 1)" 5
echo "--- after opening $FIRST:"; items

if [ -f "$REAL" ]; then
  ENC="$(printf %s "$SCR/a" | sed 's/[^A-Za-z0-9]/-/g')"
  mkdir -p "$CLAUDE_CONFIG_DIR/projects/$ENC"
  cp "$REAL" "$CLAUDE_CONFIG_DIR/projects/$ENC/"
  ID="$(basename "$REAL" .jsonl)"
  w agent resume "$P" "$ID" >/dev/null
  sleep 3
  echo "--- real transcript $ID:"; items
  echo "agent rows: $(items | grep -c '^agent ')  work sections: $(items | grep -c '^work ')"
  app_post "/window/snapshot?path=$SHOTS/chat-sections-real.png" >/dev/null && echo "shot $SHOTS/chat-sections-real.png"
else
  echo "skipped: no real transcript at $REAL"
fi
app_post "/window/snapshot?path=$SHOTS/chat-sections-fake.png" >/dev/null && echo "shot $SHOTS/chat-sections-fake.png"
finish
