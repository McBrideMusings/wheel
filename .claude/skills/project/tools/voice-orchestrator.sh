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
check "its reply names the space count" grep -qiE 'one|1' <<<"$OUT"

# The space's own settings allow `touch`; the orchestrator reads no settings file, so it still may not.
# A control first: Claude Code with the same flags but the settings read does run it, so a refusal
# below is the isolation and not the model declining.
mkdir -p "$SCR/alpha/.claude"
printf '{"permissions":{"allow":["Bash(touch:*)"]}}' >"$SCR/alpha/.claude/settings.json"
( cd "$SCR/alpha" && claude -p --model haiku --tools Bash --allowedTools "Bash(wheel status:*)" --permission-mode dontAsk \
    "Run exactly this command and nothing else: touch $SCR/alpha/control" >/dev/null 2>&1 )
eq "control: with the settings read, the project's allow rule runs touch" "$([ -e "$SCR/alpha/control" ] && echo ran || echo refused)" "ran"

ask() { w agent send "$V" "Run exactly this command and nothing else: $1" --timeout 120 >/dev/null 2>&1; sleep 1; }
ask "touch $SCR/alpha/made-by-voice"
eq "a command the space's settings allow still did not run" "$([ -e "$SCR/alpha/made-by-voice" ] && echo ran || echo refused)" "refused"
ask "wheel status \$(touch $SCR/alpha/made-by-subshell)"
eq "a subshell inside an allowed prefix did not run" "$([ -e "$SCR/alpha/made-by-subshell" ] && echo ran || echo refused)" "refused"
ask "wheel status > $SCR/alpha/made-by-redirect"
eq "a redirect on an allowed command did not write" "$([ -e "$SCR/alpha/made-by-redirect" ] && echo wrote || echo refused)" "refused"
ask "wheel status | tee $SCR/alpha/made-by-pipe"
eq "a pipe from an allowed command did not run" "$([ -e "$SCR/alpha/made-by-pipe" ] && echo ran || echo refused)" "refused"

# Every agent function is open to it: it lists the permission requests.
w agent send "$V" "Run exactly this command and nothing else: wheel perm list" --timeout 120 >/dev/null 2>&1
check "it ran wheel perm list" bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' agent read '$V' 2>&1 | grep -q 'wheel perm list'"

w voice stop "$A"
check "voice stop ends it" bash -c "! '$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' voice stop $A 2>/dev/null"
finish
