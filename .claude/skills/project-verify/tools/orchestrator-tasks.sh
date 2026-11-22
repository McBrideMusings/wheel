#!/usr/bin/env bash
# The global orchestrator folder's tools/on-event (wheel-eg4e.20.2), run by a scratch wheeld against
# the fake agent: the script in ~/.wheel/orchestrator/tools (ORCHESTRATOR_TOOLS overrides) is copied
# into the scratch Orchestrator space's folder. A message sent from the orchestrator's pane makes
# tasks/<id>.json; the child waiting on a permission makes it needs-decision, and the script's
# `wheel push send` reaches wheeld, which refuses it because the scratch wheel.toml has no push key, so the task keeps
# the refusal.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
trap cleanup EXIT

TOOLS="${ORCHESTRATOR_TOOLS:-$HOME/.wheel/orchestrator/tools}"
[ -x "$TOOLS/on-event" ] || { echo "FAIL no $TOOLS/on-event"; exit 1; }
use_fake_agent
start_daemon --state-file "$SCR/state.json"
mkdir -p "$SCR/alpha"
A="$(w space new "$SCR/alpha")"
PANE_A="$(w space ls | jq -r --arg s "$A" '[.spaces[] | select(.id == $s)][0] | [.. | objects | select(.id? and .kind?) | .id] | first')"
person() { env -u WHEEL_PANE "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$INST/token" "$@"; }

DIR="$INST/orchestrator"
mkdir -p "$DIR/tools"
cp "$TOOLS/on-event" "$DIR/tools/on-event"
V="$(person orchestrator start --global)"
person agent wait "$V" --timeout 30 >/dev/null 2>&1
person pane start "$PANE_A" agent --message hello >/dev/null
CHILD="$(person agent wait "$PANE_A" --timeout 30 | awk '{print $2}')"

WHEEL_PANE="$V" "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$INST/token" agent send "$CHILD" "Fix the login page: needs permission" --timeout 0 >/dev/null 2>&1
task() { cat "$DIR"/tasks/*.json 2>/dev/null; }
poll 20 bash -c "cat '$DIR'/tasks/*.json 2>/dev/null | jq -e '.status == \"needs-decision\" and .push_error != null' >/dev/null"
echo "task:"; task
eq "the dispatch is one task" "$(ls "$DIR/tasks" | wc -l | tr -d ' ')" "1"
eq "it holds what was asked and the child" "$(task | jq -c '[.asked, .agents]')" "[\"Fix the login page: needs permission\",[\"$CHILD\"]]"
eq "the child waiting on a permission makes it need a decision" "$(task | jq -r .status)" "needs-decision"
check "its push reached wheeld, which refused it (no push key here)" bash -c "cat '$DIR'/tasks/*.json | jq -r .push_error | grep -q 'daemon: push.apns.key is not set'"
check "wheeld logged the push send for the task" grep -rq "push send" "$LOGS"
eq "and no failed tools/on-event run" "$(cat "$DIR"/inbox/new/*.json "$DIR"/inbox/cur/*.json 2>/dev/null | jq -r 'select(.kind == "on_event_failed") | .text' | wc -l | tr -d ' ')" "0"
person orchestrator stop "$V"
finish
