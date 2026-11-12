#!/usr/bin/env bash
# § Tool rows (wheel-h3z5.12): every tool row in the chat shows an icon for its kind of action, a
# sentence, +added −removed counts for an edit and a short reason for a failure. The fake agent's
# "show tools" turn emits one of each kind, a failed command and an edit; /state lists each row's
# icon, label and counts. Drives the real Mac app through its debug server against this script's
# own daemon. Window shots (cacheDisplay) go to $WHEEL_SHOTS (default $SCR/shots), light and dark.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"

export CLAUDE_CONFIG_DIR="$SCR/claude"
use_fake_agent
printf '[agent.codex]\ncommand = "%s"\n' "$FAKE_AGENT" >>"$INST/wheel.toml"
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
rows() { app_get /state | jq -r --arg p "$P" '.panes[$p].transcript_tools[]?'; }
check "the turn's rows reached the app" wait_st ".panes[\"$P\"].transcript_tools | map(select(startswith(\"tool: \"))) | length == 11" 10
echo "--- tool rows:"; rows | grep '^tool: '

has() { rows | grep -F -- "$1" >/dev/null; }
check "Read: doc.text, 'Read model.go'" has 'icon doc.text · label "Read model.go"'
check "Grep: magnifyingglass, names the file" has 'icon magnifyingglass · label "Searched for addSuggestions in model_test.go"'
check "Glob: a search sentence" has 'label "Searched for files matching **/*.go"'
check "Edit: pencil, +2 -1" has 'icon pencil · label "Edited styles.go" · +2 -1'
check "Write: doc.badge.plus, +2 -0" has 'icon doc.badge.plus · label "Created new.go" · +2 -0'
check "a described command reads as its description" has 'icon terminal · label "Run the parser'"'"'s tests"'
check "a failed command keeps its icon and shows its first line" has 'make build (failed) · icon terminal · label "Ran make build" · reason "make: *** [build] Error 2"'
check "WebFetch: globe" has 'icon globe · label "Fetched https://example.com/doc"'
check "Skill: wand.and.stars" has 'icon wand.and.stars · label "Used the implement skill"'
check "Task: person.2" has 'icon person.2 · label "Survey the parser"'
check "MCP tool: puzzlepiece.extension" has 'icon puzzlepiece.extension · label "github: list_issues"'
check "the turn's work line totals the lines" wait_st ".panes[\"$P\"].transcript_items | map(select(startswith(\"work \") and contains(\" +4 -1 \"))) | length == 1" 5

# Codex reports a change set that only removes a file as a delete: a trash icon and "Deleted old.rs".
PC="$(w pane split "$P" right)"
w pane start "$PC" agent --agent codex >/dev/null
w agent send "$PC" "delete" >/dev/null
w agent wait "$PC" >/dev/null
check "Codex delete: trash, 'Deleted old.rs'" wait_st ".panes[\"$PC\"].transcript_tools | map(select(contains(\"icon trash · label \\\"Deleted old.rs\\\"\"))) | length == 1" 10

FIRST="$(app_get /state | jq -r --arg p "$P" '.panes[$p].transcript_items[] | select(startswith("work "))' | head -1 | sed 's/^work [0-9]* [^@]*@\(-\{0,1\}[0-9]*\).*/\1/')"
S="$(w space ls | jq -r --arg p "$P" '[.. | objects | select(.id? == $p)][0] | .agent')"
app_post "/agent/expand?agent=$S&id=$FIRST" >/dev/null || echo "expand refused"
sleep 1
for look in light dark; do
  WHEEL_APPEARANCE=$look
  quit_app
  launch_app
  app_post "/focus?space=$A" >/dev/null
  wait_st ".panes[\"$P\"].transcript_tools | length == 11" 20
  app_post "/agent/expand?agent=$S&id=$FIRST" >/dev/null || echo "expand refused"
  sleep 1
  app_post "/window/snapshot?path=$SHOTS/tool-rows-$look.png" >/dev/null && echo "shot $SHOTS/tool-rows-$look.png"
done
finish
