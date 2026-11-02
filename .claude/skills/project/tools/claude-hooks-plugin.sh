#!/usr/bin/env bash
# § 2b terminal agents: Wheel's Claude Code hooks still report when the caller passes its own
# `--settings`, because the wrapper loads them as a plugin (`--plugin-dir`), not a settings file
# (wheel-o8hq.4); and a waiting agent's reason reaches the pane's status: a Bash permission and an
# AskUserQuestion question (wheel-o8hq.3).
#
# Run it by name; all.sh does not. It sends three short prompts on the Claude account in
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

w term send "$P" "claude --model haiku --permission-mode default --settings $SCR/caller.json" --enter
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

# A permission waits: the pane says needs_you with the tool and its command, and so does its space.
status_has() { w space status 2>/dev/null | grep -qF "$1"; }
w term send "$P" "Run this exact shell command with the Bash tool and nothing else: dd if=/dev/zero of=made-by-wheel bs=1 count=1" --enter
if poll 60 status_has "needs_you  waiting 1  Bash: dd if=/dev/zero of=made-by-wheel bs=1 count=1"; then ok "a Bash permission reports its command as the reason"
else fail "a Bash permission reports its command as the reason" "$(w space status 2>&1)"; fi
if w space status 2>/dev/null | head -1 | grep -qF "Bash: dd if=/dev/zero of=made-by-wheel bs=1 count=1"; then ok "the space carries its pane's reason"
else fail "the space carries its pane's reason" "$(w space status 2>&1 | head -1)"; fi
# Refuse it: Esc ends the turn, and the reason goes with the waiting.
w term send "$P" $'\e'
if poll 20 status_has "  terminal  idle"; then ok "refusing leaves the pane idle with no reason"
else fail "refusing leaves the pane idle with no reason" "$(w space status 2>&1)"; fi

# A question waits too: Claude Code shows it as a permission dialog, so it reports the question.
w term send "$P" "Use the AskUserQuestion tool to ask me exactly this one question: Red or blue? Do nothing else." --enter
if poll 60 status_has "needs_you  waiting 1  asks: Red or blue?"; then ok "an AskUserQuestion question reports the question as the reason"
else fail "an AskUserQuestion question reports the question as the reason" "$(w space status 2>&1)"; fi
has_log 'terminal agent reported.*new=NeedsYou reason=Some\("Bash: dd if=/dev/zero of=made-by-wheel bs=1 count=1"\)' "the log has the reported reason"
w term send "$P" $'\e'

w term send "$P" "/exit" --enter
finish
