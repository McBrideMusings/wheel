#!/usr/bin/env bash
# § 3c-view-real: one pane switched between real Claude Code in a terminal and wheel's chat over
# real `claude --print`.
#
# Run it by name; all.sh does not. It sends four short prompts on the Claude account in
# ~/.claude (or $CLAUDE_CONFIG_DIR), and starts wheeld with the real agent command.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# Run from inside a Claude Code session, the environment carries that session's markers, and a
# Claude Code started under it does not save its transcript, so the daemon never learns the
# conversation. The programs under test start as they would from a plain terminal.
unset CLAUDECODE CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDE_CODE_ENTRYPOINT \
  CLAUDE_CODE_EXECPATH CLAUDE_CODE_SESSION_ATTENDED CLAUDE_CODE_MESSAGING_SOCKET \
  CLAUDE_CODE_MESSAGING_TOKEN CLAUDE_PID

command -v claude >/dev/null || { echo "needs claude on PATH" >&2; exit 2; }

now() { perl -MTime::HiRes=time -e 'printf "%.3f", time'; }
tail_screen() { w term read "$P" --timeout 0 2>&1 | tail -15; }

PROJ="$SCR/proj"
mkdir -p "$PROJ"
start_daemon --state-file "$SCR/state.json"

w space new "$PROJ" >/dev/null
P="$(w space ls | jget spaces.0.root.id)"
w pane start "$P" terminal >/dev/null

# Claude Code asks once whether it may work in a directory it has not seen.
w term send "$P" claude --enter
if w term read "$P" --until "trust this folder" --timeout 20 >/dev/null 2>&1; then
  w term send "$P" "" --enter
fi
if w term read "$P" --until "❯" --timeout 30 >/dev/null 2>&1; then ok "Claude Code shows its prompt"
else fail "Claude Code shows its prompt" "$(tail_screen)"; finish; fi

w term send "$P" "reply with the word ok" --enter
if w term read "$P" --new --until "⏺ ok" --timeout 60 >/dev/null 2>&1; then ok "Claude Code answers the first prompt"
else fail "Claude Code answers the first prompt" "$(tail_screen)"; finish; fi
sleep 3   # the daemon's 2 s sampler learns the conversation id

# Terminal -> chat: Claude Code must exit within the daemon's TUI_EXIT (10 s).
t0="$(now)"
if out="$(w pane view "$P" agent 2>&1)"; then t1="$(now)"
  ok "pane view agent succeeds: $out"
  secs="$(perl -e "printf '%.1f', $t1 - $t0")"
  if perl -e "exit !($t1 - $t0 < 10)"; then ok "Claude Code left within 10 s (${secs} s)"
  else fail "Claude Code left within 10 s" "took ${secs} s"; fi
else fail "pane view agent succeeds" "$out"; finish; fi

# The pane id names the chat agent whichever session the last switch gave it.
AGENT="$P"
w agent wait "$AGENT" --timeout 60 >/dev/null 2>&1
read_agent() { w agent read "$AGENT" 2>&1; }

# The chat agent loaded the session the TUI wrote.
conv="$(read_agent)"
if printf %s "$conv" | grep -q "reply with the word ok" && printf %s "$conv" | grep -v "reply with the word" | grep -Eqi "(^|[^a-z])ok([^a-z]|$)"; then
  ok "agent read holds the TUI's prompt and its reply"
else fail "agent read holds the TUI's prompt and its reply" "$conv"; fi

if out="$(w agent send "$AGENT" "reply with the word two" 2>&1)" && printf %s "$out" | grep -v "reply with the word" | grep -qi two; then
  ok "the chat agent continues the TUI's conversation"
else fail "the chat agent continues the TUI's conversation" "$out"; fi

# Chat -> terminal: claude --resume shows both exchanges.
if out="$(w pane view "$P" terminal 2>&1)"; then ok "pane view terminal succeeds: $out"
else fail "pane view terminal succeeds" "$out"; finish; fi
if w term read "$P" --until "reply with the word two" --timeout 45 >/dev/null 2>&1; then
  screen="$(w term read "$P" --timeout 0 2>&1)"
  if printf %s "$screen" | grep -q "reply with the word ok"; then ok "claude --resume shows both exchanges"
  else fail "claude --resume shows both exchanges" "$(printf %s "$screen" | tail -15)"; fi
else fail "claude --resume shows the chat's exchange" "$(tail_screen)"; fi

# A switch during a turn is refused.
sleep 8   # claude --resume finishes loading
w term send "$P" "count slowly from 1 to 30, one number per line, and say nothing else"
sleep 1
w term send "$P" "" --enter
turn_running() { w term read "$P" --timeout 0 2>&1 | grep -q 'tokens)'; }
poll 20 turn_running || { fail "the counting turn started" "$(tail_screen)"; finish; }
out="$(w pane view "$P" agent 2>&1)"; code=$?
if [ "$code" != 0 ] && printf %s "$out" | grep -q "in the middle of a turn"; then ok "pane view agent during a turn is refused: $out"
else fail "pane view agent during a turn is refused" "exit $code: $out"; fi
[ "$code" = 0 ] && { w agent wait "$AGENT" --timeout 60 >/dev/null 2>&1; w pane view "$P" terminal >/dev/null 2>&1; sleep 8; }

# After the turn it succeeds: the Stop hook reported idle.
last_number_shown() { w term read "$P" --timeout 0 2>&1 | perl -pe 's/\e\[[0-9;?]*[A-Za-z]//g' | tr -d '\r' | grep -Eq '^ *30[^0-9a-zA-Z,]*$'; }
if poll 90 last_number_shown; then
  sleep 5
  out="$(w pane view "$P" agent 2>&1)"; code=$?
  if [ "$code" = 0 ]; then ok "pane view agent after the turn ends succeeds: $out"
  else fail "pane view agent after the turn ends succeeds" "exit $code: $out"; fi
else fail "the counting turn ends" "$(tail_screen)"; fi

# Race: type a prompt just after the switch starts. Reported, not judged.
w agent wait "$AGENT" --timeout 60 >/dev/null 2>&1
out="$(w pane view "$P" terminal 2>&1)" || { fail "pane view terminal before the race" "$out"; finish; }
sleep 8
w pane view "$P" agent >"$SCR/race-view.out" 2>&1 &
RV=$!; BG_PIDS+=("$RV")
w term send "$P" "reply with the word three" >/dev/null 2>&1
w term send "$P" "" --enter >/dev/null 2>&1
wait "$RV"; rcode=$?
w agent wait "$AGENT" --timeout 60 >/dev/null 2>&1
sleep 3
conv="$(w agent read "$AGENT" 2>&1)"
if printf %s "$conv" | grep -q "reply with the word three"; then seen=yes; else seen=no; fi
if printf %s "$conv" | grep -v "reply with the word" | grep -Eqi "(^|[^a-z])three([^a-z]|$)"; then word=yes; else word=no; fi
echo "race: switch exit $rcode (\"$(cat "$SCR/race-view.out")\"); the typed prompt is in agent read: $seen; a reply with \"three\" is in agent read: $word"

finish
