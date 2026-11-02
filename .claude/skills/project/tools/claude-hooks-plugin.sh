#!/usr/bin/env bash
# § 2b terminal agents: Wheel's Claude Code hooks still report when the caller passes its own
# `--settings`, because the wrapper loads them as a plugin (`--plugin-dir`), not a settings file.
#
# Run it by name; all.sh does not. It sends one short prompt on the Claude account in
# ~/.claude (or $CLAUDE_CONFIG_DIR).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

unset CLAUDECODE CLAUDE_CODE_CHILD_AGENT CLAUDE_CODE_AGENT_ID CLAUDE_CODE_ENTRYPOINT \
  CLAUDE_CODE_EXECPATH CLAUDE_CODE_AGENT_ATTENDED CLAUDE_CODE_MESSAGING_SOCKET \
  CLAUDE_CODE_MESSAGING_TOKEN CLAUDE_PID

command -v claude >/dev/null || { echo "needs claude on PATH" >&2; exit 2; }
tail_screen() { w term read "$P" --timeout 0 2>&1 | tail -15; }

PROJ="$SCR/proj"
mkdir -p "$PROJ"
# The caller's own settings, as a dispatched worker passes them: a status line of its own.
printf '{"statusLine":{"type":"command","command":"touch %s/caller-status-ran; echo caller"}}' "$SCR" >"$SCR/caller.json"
start_daemon --state-file "$SCR/state.json"

w space new "$PROJ" >/dev/null
P="$(w space ls | jget spaces.0.root.id)"
w pane start "$P" terminal >/dev/null

w term send "$P" "claude --model haiku --settings $SCR/caller.json" --enter
if w term read "$P" --until "trust this folder" --timeout 20 >/dev/null 2>&1; then
  w term send "$P" "" --enter
fi
if w term read "$P" --until "❯" --timeout 60 >/dev/null 2>&1; then ok "Claude Code shows its prompt"
else fail "Claude Code shows its prompt" "$(tail_screen)"; finish; fi

sleep 3   # the daemon's 2 s sampler sees the agent; a report before that is ignored
w term send "$P" "reply with the word ok" --enter
if w term read "$P" --new --until "⏺ ok" --timeout 60 >/dev/null 2>&1; then ok "Claude Code answers"
else fail "Claude Code answers" "$(tail_screen)"; finish; fi

if poll 10 grep -q "terminal agent reported.*new=Working" "$(LOG)"; then ok "UserPromptSubmit reported working despite the caller's --settings"
else fail "UserPromptSubmit reported working despite the caller's --settings" "$(grep 'terminal agent' "$(LOG)" | tail -3)"; fi
if poll 10 grep -q "terminal agent reported.*new=Idle" "$(LOG)"; then ok "Stop reported idle despite the caller's --settings"
else fail "Stop reported idle despite the caller's --settings" "$(grep 'terminal agent' "$(LOG)" | tail -3)"; fi
if [ -e "$SCR/caller-status-ran" ]; then ok "the caller's own status line runs"
else fail "the caller's own status line runs"; fi

w term send "$P" "/exit" --enter
finish
