#!/usr/bin/env bash
# Orchestrator approvals (wheel-e1xk.10). The orchestrator runs `wheel perm answer` with WHEEL_ORCHESTRATOR
# set; for a request that cannot be undone (a push, a delete, a restart) the command refuses to
# allow it until it is run again with --confirmed, which the orchestrator passes only after the
# person said yes out loud. A request it may allow, and any denial, go through unasked, and the
# same command without WHEEL_ORCHESTRATOR is not gated.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
trap cleanup EXIT

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
mkdir -p "$SCR/proj"
S="$(w space new "$SCR/proj")"
P="$(w space ls | jq -r --arg s "$S" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
w pane start "$P" agent >/dev/null
AGENT="$(w status --json | jq -r --arg p "$P" '[.spaces[].root | .. | objects | select(.id == $p)] | .[0].agent')"
bg w agent send "$AGENT" "permission twice"
waiting() { w perm list | jq -r '.[0].id // empty'; }
check "a request that deletes is waiting" poll 20 bash -c "[ -n \"\$($WHEEL --url ws://127.0.0.1:$PORT/ws --token-file $INST/token perm list | jq -r '.[0].id // empty')\" ]"
Q="$(waiting)"
eq "it is the rm request" "$(w perm list | jq -r '.[0].title' | grep -c 'rm -rf build')" "1"

orchestrator() { WHEEL_ORCHESTRATOR=1 w perm answer "$@"; }
OUT="$(orchestrator "$Q" allow 2>&1)"; CODE=$?
check "allowing it without --confirmed is refused" bash -c "[ $CODE -ne 0 ]"
check "the refusal says to ask out loud and use --confirmed" bash -c "echo '$OUT' | grep -q 'ask the person out loud'"
eq "the request is still waiting" "$(waiting)" "$Q"
check "allowing it with --confirmed goes through" orchestrator "$Q" allow --confirmed
check "the next request (rm -rf dist) is waiting" poll 20 bash -c "[ \"\$($WHEEL --url ws://127.0.0.1:$PORT/ws --token-file $INST/token perm list | jq -r '.[0].title // empty')\" = 'rm -rf dist' ] || $WHEEL --url ws://127.0.0.1:$PORT/ws --token-file $INST/token perm list | jq -e '.[0].id' >/dev/null"
Q2="$(waiting)"
check "a denial needs no confirmation" orchestrator "$Q2" deny
w agent wait "$AGENT" --timeout 20 >/dev/null 2>&1
bg w agent send "$AGENT" "permission"
check "a third request is waiting" poll 20 bash -c "$WHEEL --url ws://127.0.0.1:$PORT/ws --token-file $INST/token perm list | jq -e '.[0].id' >/dev/null"
Q3="$(waiting)"
check "without WHEEL_ORCHESTRATOR the same answer is not gated" w perm answer "$Q3" allow
finish
