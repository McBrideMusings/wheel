#!/usr/bin/env bash
# § Background footer: while an agent has background tasks running, the line under its composer
# says how many, `1 shell · 2 agents`, and says nothing at zero. The fake agent's "start background"
# turn reports one shell, two subagents and one ambient monitor (not counted); "end background"
# reports none. Drives the real Mac app through its debug server against this script's own daemon;
# /state lists the footer as `footer` on the pane. Window shots go to $WHEEL_SHOTS (default $SCR/shots).
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

check "no task runs at the start: the footer reads none" wait_st ".panes[\"$P\"].footer.background_text == null and .panes[\"$P\"].footer.shells == 0" 20
shot none

w agent send --timeout 0 "$P" "start background" >/dev/null
check "one shell and two agents run: the footer reads them" wait_st ".panes[\"$P\"].footer.background_text == \"1 shell · 2 agents\"" 20
echo "     $(st ".panes[\"$P\"].footer")"
shot running

w agent send --timeout 0 "$P" "end background" >/dev/null
check "the tasks end: the footer reads none again" wait_st ".panes[\"$P\"].footer.background_text == null and .panes[\"$P\"].footer.shells == 0" 20
shot ended

finish
