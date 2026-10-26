#!/usr/bin/env bash
# § 2b-context: a terminal pane shows the context window its agent reports.
#
# Run it by name; all.sh does not. The first half needs no model: `wheel pane statusline` reports
# Claude Code's status-line input and prints the person's own status line for it. The second half
# runs real Claude Code in a terminal pane (one short prompt on the account in ~/.claude, or
# $CLAUDE_CONFIG_DIR) with a project status line that saves its input, and compares the pane's
# window with the one in that input.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

unset CLAUDECODE CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDE_CODE_ENTRYPOINT \
  CLAUDE_CODE_EXECPATH CLAUDE_CODE_SESSION_ATTENDED CLAUDE_CODE_MESSAGING_SOCKET \
  CLAUDE_CODE_MESSAGING_TOKEN CLAUDE_PID

INPUT='{"cwd":"/tmp","context_window":{"context_window_size":1000000,"current_usage":{"input_tokens":2,"cache_creation_input_tokens":10,"cache_read_input_tokens":261000,"output_tokens":255}}}'

mkdir -p "$SCR/cfg"
check "no status line configured prints nothing" bash -c \
  "[ -z \"\$(printf %s '$INPUT' | CLAUDE_CONFIG_DIR='$SCR/cfg' '$WHEEL' pane statusline --pane '')\" ]"
printf '{"statusLine":{"type":"command","command":"printf \\"LINE:%%s\\" \\"$(cat)\\""}}' >"$SCR/cfg/settings.json"
got="$(printf %s "$INPUT" | CLAUDE_CONFIG_DIR="$SCR/cfg" "$WHEEL" pane statusline --pane '')"
eq "the person's status line gets the same input and prints its own text" "$got" "LINE:$INPUT"

command -v claude >/dev/null || { echo "needs claude on PATH for the real-agent half" >&2; finish; }

PROJ="$SCR/proj"
mkdir -p "$PROJ/.claude"
printf '{"statusLine":{"type":"command","command":"cat > %s/statusline-input.json"}}' "$SCR" >"$PROJ/.claude/settings.local.json"
start_daemon --state-file "$SCR/state.json" --cli "$WHEEL"
w space new "$PROJ" >/dev/null
P="$(w space ls | jget spaces.0.root.id)"
w pane start "$P" terminal >/dev/null
w term send "$P" claude --enter
if w term read "$P" --until "trust this folder" --timeout 20 >/dev/null 2>&1; then
  w term send "$P" "" --enter
fi
if w term read "$P" --until "❯" --timeout 30 >/dev/null 2>&1; then ok "Claude Code shows its prompt"
else fail "Claude Code shows its prompt" "$(w term read "$P" --timeout 0 2>&1 | tail -15)"; finish; fi
w term send "$P" "reply with the word ok" --enter
if w term read "$P" --new --until "⏺ ok" --timeout 60 >/dev/null 2>&1; then ok "Claude Code answers"
else fail "Claude Code answers" "$(w term read "$P" --timeout 0 2>&1 | tail -15)"; finish; fi
sleep 6   # the status line runs after the reply; the daemon's 2 s sampler applies the report

[ -f "$SCR/statusline-input.json" ] || { fail "Claude Code ran Wheel's status line" "no $SCR/statusline-input.json"; finish; }
want="$(jget context_window.context_window_size <"$SCR/statusline-input.json")"
size="$(w space ls | jget spaces.0.root.agent.context.size)"
used="$(w space ls | jget spaces.0.root.agent.context.used)"
eq "the pane's window is the one Claude Code's status line input reports ($want)" "$size" "$want"
check "the pane shows tokens in use ($used)" test "${used:-0}" -gt 0
finish
