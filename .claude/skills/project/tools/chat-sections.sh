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

# A running turn: the long message, the tool call after it and the closing words, as they stream.
MSGS="$(st ".panes[\"$P\"].transcript_messages | length")"
w agent send "$P" "write mid-turn" >/dev/null
check "a running turn already draws its first message as a row of its own" \
  wait_st ".panes[\"$P\"].transcript_messages | length == $((MSGS + 1))" 10
w agent wait "$P" >/dev/null
echo "--- streamed turn:"; items | tail -4
check "the streamed turn reads message, section, message, with the section collapsed" \
  wait_st ".panes[\"$P\"].transcript_items | .[-3:] | (map(split(\" \")[0]) == [\"agent\", \"work\", \"agent\"]) and (map(contains(\" open\")) | any | not)" 5

# A relaunch: the conversation is drawn again from the daemon and every section is collapsed.
BEFORE="$(items)"
quit_app
launch_app
app_post "/focus?space=$A" >/dev/null
same_after_relaunch() { wait_st ".panes[\"$P\"].transcript_items | length >= 1" 20; local want; want="$(echo "$BEFORE" | sed 's/ open//')"; [ "$(items)" = "$want" ] || { diff <(echo "$want") <(items); return 1; }; }
check "after a relaunch the conversation has the same rows and sections" same_after_relaunch
check "after a relaunch every section is collapsed" \
  wait_st ".panes[\"$P\"].transcript_items | map(select(startswith(\"work \") and contains(\" open\"))) | length == 0" 5
echo "--- after relaunch:"; items

if [ -f "$REAL" ]; then
  ENC="$(printf %s "$SCR/a" | sed 's/[^A-Za-z0-9]/-/g')"
  mkdir -p "$CLAUDE_CONFIG_DIR/projects/$ENC"
  cp "$REAL" "$CLAUDE_CONFIG_DIR/projects/$ENC/"
  ID="$(basename "$REAL" .jsonl)"
  w agent resume "$P" "$ID" >/dev/null
  sleep 3
  echo "--- real transcript $ID:"; items
  msgs() { app_get /state | jq -c --arg p "$P" '.panes[$p].transcript_messages[]'; }
  msgs
  WANT="$(jq -rs '[.[] | select(.type == "assistant" and .isSidechain != true) | .message.content[]? | select(.type == "text" and (.text | gsub("\\s"; "") | length) > 0)] | length' "$REAL")"
  check "every assistant text block of the file ($WANT) is a message row, none lost to a tail" \
    wait_st ".panes[\"$P\"].transcript_messages | length == $WANT" 5
  check "the mid-turn message 'Found a 33 GB PS4 rip' is a row of the main chat" \
    wait_st ".panes[\"$P\"].transcript_messages | map(select(.head | startswith(\"Found a 33 GB\"))) | length == 1" 5
  check "the Steam Deck answer (3,151 chars) is a row of the main chat" \
    wait_st ".panes[\"$P\"].transcript_messages | map(select((.head | startswith(\"Yes, it can be done\")) and .chars >= 3000)) | length >= 1" 5
  check "no work section holds an agent message" \
    wait_st ".panes[\"$P\"] | (.transcript_messages | map(.row)) as \$m | .transcript_items | map(select(startswith(\"work \")) | capture(\"\\\\[(?<ids>[^\\\\]]*)\\\\]\").ids | split(\",\") | map(split(\" \")[0] | tonumber)) | flatten | map(select(. as \$i | \$m | index(\$i))) | length == 0" 5
  echo "agent rows: $(items | grep -c '^agent ')  work sections: $(items | grep -c '^work ')  items: $(items | wc -l)"
  app_post "/window/snapshot?path=$SHOTS/chat-sections-real.png" >/dev/null && echo "shot $SHOTS/chat-sections-real.png"
else
  echo "skipped: no real transcript at $REAL"
fi
app_post "/window/snapshot?path=$SHOTS/chat-sections-fake.png" >/dev/null && echo "shot $SHOTS/chat-sections-fake.png"
finish
