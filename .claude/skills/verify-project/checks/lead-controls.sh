#!/usr/bin/env bash
# § 3b-saved The lead's model, effort, fast mode and permission mode are saved and come back.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

STATE="$SCR/spaces.json"
start_daemon --lead-cmd "$FAKE_AGENT" --lead-cwd /tmp --state-file "$STATE"
lead_ready() { [ "$(w status | jget lead.phase)" = ready ]; }
# opt <id>: the current value of one of the lead's controls.
opt() { w lead config | sed -n "s/^$1 = \([^ ]*\) .*/\1/p"; }
poll 30 lead_ready || fail "the lead is ready" "$(w status | jget lead.phase)"

eq "nothing saved: mode is auto" "$(opt mode)" "auto"
eq "...model is the agent's own" "$(opt model)" "default"
check "...and no machine file was written" test ! -e "$INST/wheel.toml"

w lead config model sonnet >/dev/null
w lead config effort high >/dev/null
w lead config fast true >/dev/null
w lead config mode plan >/dev/null
eq "lead.model is saved" "$(w config get lead.model)" "sonnet"
eq "lead.effort is saved" "$(w config get lead.effort)" "high"
eq "lead.fast is saved" "$(w config get lead.fast)" "true"
eq "lead.mode is saved" "$(w config get lead.mode)" "plan"
check "...in the instance's wheel.toml" grep -q '^model = "sonnet"' "$INST/wheel.toml"
check "the saved layout holds none of them" file_lacks "sonnet" "$STATE"

restart_daemon
poll 30 lead_ready || fail "the lead is ready after a restart" "$(w status | jget lead.phase)"
eq "after a restart: model" "$(opt model)" "sonnet"
eq "...effort" "$(opt effort)" "high"
eq "...fast" "$(opt fast)" "true"
eq "...mode" "$(opt mode)" "plan"
has_log 'session started with config.*model=sonnet.*effort=high.*fast=true.*mode=plan' "log: session started with config carries all four"

w lead new >/dev/null
eq "a new conversation keeps the mode" "$(opt mode)" "plan"

# A saved value the agent refuses is a problem on that key, and the agent keeps its own.
w config set lead.model gone --machine >/dev/null
restart_daemon
poll 30 lead_ready || fail "the lead is ready after a bad value" "$(w status | jget lead.phase)"
eq "a refused model leaves the agent's own" "$(opt model)" "default"
case "$(w config ls --json)" in *'the agent refused'*) ok "...and lead.model reports a problem";; *) fail "lead.model problem";; esac
has_log 'the agent refused a saved value' "log: the agent refused a saved value"
finish
