#!/usr/bin/env bash
# § The brief lists the artifacts that exist when a session starts (wheel-ci6h, ADR-0018): a chat
# agent and a terminal agent in a space with two artifacts both see both names in their system
# prompt; with 30 artifacts the section shows 20 and the count; with long descriptions the section
# stays within 2000 bytes.
#
# Run it by name; all.sh does not. The terminal half runs real Claude Code once (`claude -p`, one
# short prompt on the account in ~/.claude, or $CLAUDE_CONFIG_DIR) inside a terminal pane, so the
# wrapper in Wheel's bin directory hands it the pane's brief file.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
unset CLAUDECODE CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDE_CODE_ENTRYPOINT \
  CLAUDE_CODE_EXECPATH CLAUDE_CODE_SESSION_ATTENDED CLAUDE_CODE_MESSAGING_SOCKET \
  CLAUDE_CODE_MESSAGING_TOKEN CLAUDE_PID

use_fake_agent
start_daemon --state-file "$SCR/state.json"
chat_brief() { # <space id> : the fake agent's brief for a chat pane started in that space
  local p
  p="$(w space ls | jq -r --arg s "$1" '[.spaces[] | select(.id == $s)][0] | [.. | objects | select(.id? and .kind?) | .id] | first')"
  w pane start "$p" agent --message "show setup" >/dev/null
  w agent wait "$p" --timeout 10 >/dev/null
  w agent read "$p"
}

# Two artifacts: chat agent.
mkdir -p "$SCR/two"
S2="$(w space new "$SCR/two")"
w artifact new alpha --pane --space "$S2" --description "First page." >/dev/null
w artifact new beta --pane --space "$S2" --description "Second page." >/dev/null
SEEN="$(chat_brief "$S2")"
echo "$SEEN" | grep -E "^artifacts:" | sed 's/^/     chat: /'
expect_out "- alpha \(project\): First page\." "chat agent brief names alpha" echo "$SEEN"
expect_out "- beta \(project\): Second page\." "chat agent brief names beta" echo "$SEEN"

# Two artifacts: terminal agent (real claude, through the wrapper).
if command -v claude >/dev/null; then
  P="$(w space ls | jq -r --arg s "$S2" '[.spaces[] | select(.id == $s)][0] | [.. | objects | select(.id? and .kind?) | .id] | first')"
  w pane kind "$P" terminal >/dev/null 2>&1 || w pane start "$P" terminal >/dev/null
  w term send "$P" "claude -p 'List the names after the dash in the Artifacts section of your system prompt, comma separated, nothing else.' > $SCR/term-answer.txt; echo done-\$((6*7))" --enter
  if w term read "$P" --until "done-42" --timeout 90 >/dev/null 2>&1; then ok "claude -p finished in the terminal pane"
  else fail "claude -p finished in the terminal pane" "$(w term read "$P" --timeout 0 2>&1 | tail -10)"; fi
  echo "     terminal answer: $(cat "$SCR/term-answer.txt" 2>/dev/null)"
  check "terminal agent's answer names alpha" grep -q alpha "$SCR/term-answer.txt"
  check "terminal agent's answer names beta" grep -q beta "$SCR/term-answer.txt"
  B="$(find "$INST" -path '*briefs/*.md' | head -1)"
  echo "     brief file: $B"
  check "the pane's brief file holds both names" bash -c "grep -q -- '- alpha' '$B' && grep -q -- '- beta' '$B'"
else
  fail "claude is on PATH for the terminal half" "not found"
fi

# Thirty artifacts with long descriptions: 20 shown, count, within 2000 bytes.
mkdir -p "$SCR/many"
S30="$(w space new "$SCR/many")"
LONG="$(printf 'x%.0s' $(seq 1 190))"
for i in $(seq -w 1 30); do w artifact new "art$i" --pane --space "$S30" --description "$LONG" >/dev/null; done
SEEN="$(chat_brief "$S30")"
SECTION="$(printf '%s\n' "$SEEN" | grep '^artifacts:' | sed 's/^artifacts: //')"
echo "$SECTION" | cut -c1-100 | sed 's/^/     many: /'
BYTES="$(printf '%s' "$SECTION" | wc -c | tr -d ' ')"
LISTED="$(printf '%s' "$SECTION" | grep -o -- '- art[0-9]* ' | wc -l | tr -d ' ')"
echo "     section bytes (joined by ' | '): $BYTES, entries listed: $LISTED"
expect_out "30 artifacts in all" "the section gives the count of 30" echo "$SECTION"
check "the section is at most 2000 bytes" test "$BYTES" -le 2000

# Thirty artifacts with short descriptions: exactly 20 shown, then the count.
mkdir -p "$SCR/short"
S30S="$(w space new "$SCR/short")"
for i in $(seq -w 1 30); do w artifact new "s$i" --pane --space "$S30S" --description "d" >/dev/null; done
SEEN="$(chat_brief "$S30S")"
SECTION="$(printf '%s\n' "$SEEN" | grep '^artifacts:' | sed 's/^artifacts: //')"
LISTED="$(printf '%s' "$SECTION" | grep -o -- '- s[0-9]* ' | wc -l | tr -d ' ')"
echo "     short descriptions: entries listed $LISTED"
eq "30 artifacts with short descriptions list 20" "$LISTED" 20
expect_out "30 artifacts in all" "and give the count of 30" echo "$SECTION"
finish
