#!/usr/bin/env bash
# § A hand edit of wheel.toml reaches the open chat and Settings without a restart (wheel-h3z5.8).
# Drives the real Mac app against this script's own daemon: with an agent pane open, `sed` edits
# chat.font.size in the machine file, then in the space's file; /state's agent `appearance.size`
# and the Settings window's row must follow within a second. A file that stops parsing keeps the
# last good size. The daemon log holds `settings file changed <path>: <keys>`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

printf '[agent.claude]\ncommand = "%s"\n[chat.font]\nsize = 14\n' "$FAKE_AGENT" >"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"
SPACE="$(mkspaces 1)"
PANE="$(w space ls | jq -r --arg s "$SPACE" '[.spaces[] | select(.id==$s) | .root | .. | objects | select(.node? == "pane") | .id][0]')"
w pane start "$PANE" agent >/dev/null
prefs_reset
launch_app
check "the app connects" wait_st '.connection == "connected"' 20
app_post "/focus?space=$SPACE" >/dev/null
check "the agent pane is drawn" wait_st '[.panes[] | select(.kind == "agent")] | length == 1' 20
app_post "/settings?action=open" >/dev/null
check "settings opens" wait_st '.settings.open and .settings.loaded' 10
size() { st --arg p "$PANE" '.panes[$p].appearance.size'; }
row() { st '.settings.rows | if type == "array" then (.[] | select(.key=="chat.font.size") | .value) else .["chat.font.size"].value end'; }
eq "the chat starts at 14" "$(size)" 14

sed -i '' 's/^size = 14/size = 19/' "$INST/wheel.toml"
check "a sed edit of the machine file moves the chat to 19 within a second" wait_st ".panes[\"$PANE\"].appearance.size == 19" 1
eq "Settings' row follows" "$(row)" 19
grep -q "settings file changed $INST/wheel.toml: chat.font.size" "$LOGS"/wheeld*.log && ok "the daemon log names the file and key" || fail "log line" "missing"

printf '[chat.font]\nsize = 21\n' >"$SCR/proj1/wheel.toml"
check "the space's own file wins within a second" wait_st ".panes[\"$PANE\"].appearance.size == 21" 1

printf '[chat.font\nsize = ' >"$INST/wheel.toml"
sleep 1
eq "a file that does not parse keeps the last good size" "$(size)" 21
printf '[chat.font]\nsize = 16\n' >"$INST/wheel.toml"
rm "$SCR/proj1/wheel.toml"
check "fixed and the project file gone, the machine's 16 shows" wait_st ".panes[\"$PANE\"].appearance.size == 16" 2
stop_daemon
finish
