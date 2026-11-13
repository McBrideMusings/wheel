#!/usr/bin/env bash
# § Background footer and running strip: while an agent has background tasks running, the line
# under its composer says how many, `1 shell · 2 agents`, and a strip above the composer has one
# row per task with its latest output line and running time. A row opens to its last lines, ⤢
# opens the full view with the output read from wheeld, and "Show in Transcript" scrolls to the
# call. The fake agent's "start background" turn reports one shell (its output file holds three
# lines), two subagents and one ambient monitor (not shown); "end background" reports none.
# Drives the real Mac app through its debug server against this script's own daemon; /state lists
# the footer and its tasks as `footer` on the pane. Window shots go to $WHEEL_SHOTS (default $SCR/shots).
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
shot() { app_post "/window/snapshot?path=$SHOTS/background-footer-$1.png" >/dev/null && echo "     shot $SHOTS/background-footer-$1.png"; }

check "no task runs at the start: no footer count, no strip" wait_st ".panes[\"$P\"].footer.background_text == null and (.panes[\"$P\"].footer.tasks | length) == 0" 20
shot none

S="$(st --arg p "$P" '.panes[$p].agent')"
w agent send --timeout 0 "$P" "start background" >/dev/null
check "one shell and two agents run: the footer reads them" wait_st ".panes[\"$P\"].footer.background_text == \"1 shell · 2 agents\"" 20
check "the shell's row has its command, call and latest line" wait_st ".panes[\"$P\"].footer.tasks[0] | .command == \"make deploy-client\" and .call == \"bg-call-1\" and .latest == \"line 3\"" 20
check "the subagent's row has its progress" wait_st ".panes[\"$P\"].footer.tasks[1].latest == \"2 tool calls · last: Bash · 26.7k tokens\"" 20
echo "     $(st ".panes[\"$P\"].footer.tasks | map({id, name, elapsed, latest})" | tr -d '\n')"
shot strip

OUT="$(st ".panes[\"$P\"].footer.tasks[0].output_bytes")"
check "the shell's output file is named" test "$OUT" = 21
app_post "/agent/task?agent=$S&task=bash-1&action=expand" >/dev/null
check "a click opens the row" wait_st ".panes[\"$P\"].footer.tasks[0].expanded == true" 10
shot row-open

app_post "/agent/task?agent=$S&task=bash-1&action=full" >/dev/null
check "the full view opens on the task and reads its output from wheeld" wait_st ".panes[\"$P\"].footer.full_task == \"bash-1\" and .panes[\"$P\"].footer.tasks[0].output_read_bytes == 21" 10
shot full

app_post "/agent/task?agent=$S&task=bash-1&action=reveal" >/dev/null
check "Show in Transcript closes the full view" wait_st ".panes[\"$P\"].footer.full_task == null" 10
check "the app log says the transcript scrolled to the call" grep -q "shows call bg-call-1" "$INST/WheelMac.log"
shot revealed

w agent send --timeout 0 "$P" "end background" >/dev/null
check "the tasks end: no footer count, no strip" wait_st ".panes[\"$P\"].footer.background_text == null and (.panes[\"$P\"].footer.tasks | length) == 0" 20
shot ended

finish
