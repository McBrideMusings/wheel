#!/usr/bin/env bash
# The voice orchestrator (wheel-e1xk.7). `wheel voice start <scope>` starts an agent that belongs to
# no pane, for a space or `daemon`; it answers what the person says by running only the whitelisted
# `wheel` commands, and `wheel voice stop` ends it.
#
# Run it by name; all.sh does not. It runs real Claude Code on the account in ~/.claude (two short
# prompts on Haiku), so it needs `claude` on PATH.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
trap cleanup EXIT

unset CLAUDECODE CLAUDE_CODE_CHILD_AGENT CLAUDE_CODE_AGENT_ID CLAUDE_CODE_ENTRYPOINT \
  CLAUDE_CODE_EXECPATH CLAUDE_CODE_AGENT_ATTENDED CLAUDE_CODE_MESSAGING_SOCKET \
  CLAUDE_CODE_MESSAGING_TOKEN CLAUDE_PID
command -v claude >/dev/null || { echo "needs claude on PATH" >&2; exit 2; }

start_daemon --state-file "$SCR/state.json"
mkdir -p "$SCR/alpha"
A="$(w space new "$SCR/alpha")"

check "a scope that names no space is refused" bash -c "! '$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' voice start s-missing 2>/dev/null"
V="$(w voice start "$A")"
eq "voice start names the orchestrator's agent" "$V" "voice-$A"
eq "a second start finds the same one" "$(w voice start "$A")" "$V"
eq "no pane holds it" "$(w space ls | grep -c "$V")" "0"

# It reads state through an allowed command.
w agent wait "$V" --timeout 60 >/dev/null 2>&1
w agent send "$V" "Run the command wheel space ls and tell me in one sentence how many spaces there are." --timeout 120 >/dev/null 2>&1
OUT="$(w agent read "$V" 2>&1)"
echo "$OUT" | tail -12
eq "it ran the allowed command" "$([ "$(echo "$OUT" | grep -c 'wheel space ls')" -ge 1 ] && echo yes)" "yes"
check "its reply names the space count" bash -c "echo '$OUT' | grep -qiE 'one|1'"

# A command outside the whitelist is refused, not run.
w agent send "$V" "Run the command: touch $SCR/alpha/made-by-voice" --timeout 120 >/dev/null 2>&1
sleep 1
eq "a command outside the whitelist did not run" "$([ -e "$SCR/alpha/made-by-voice" ] && echo ran || echo refused)" "refused"

w voice stop "$A"
check "voice stop ends it" bash -c "! '$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' voice stop $A 2>/dev/null"
finish
