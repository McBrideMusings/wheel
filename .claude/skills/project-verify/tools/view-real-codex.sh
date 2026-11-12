#!/usr/bin/env bash
# § 3c-view-real-codex: one pane switched between real Codex in a terminal and wheel's chat over
# real `codex app-server`.
#
# Run it by name; all.sh does not. It sends a few short prompts on the Codex login in
# ~/.codex (or $CODEX_HOME), and starts wheeld with the real agent command.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

command -v codex >/dev/null || { echo "needs codex on PATH" >&2; exit 2; }

now() { perl -MTime::HiRes=time -e 'printf "%.3f", time'; }
clean() { perl -pe 's/\e\[[0-9;?]*[A-Za-z]//g' | tr -d '\r'; }
tail_screen() { w term read "$P" --timeout 0 2>&1 | clean | tail -15; }

# A Codex home of its own: the login is linked in, and the project directory is trusted up front,
# so Codex shows neither its hooks review nor its trust question and the person's own ~/.codex is
# not written to.
SRC_HOME="${CODEX_HOME:-$HOME/.codex}"
[ -f "$SRC_HOME/auth.json" ] || { echo "needs a Codex login in $SRC_HOME" >&2; exit 2; }
export CODEX_HOME="$SCR/codex-home"
mkdir -p "$CODEX_HOME"
ln -s "$SRC_HOME/auth.json" "$CODEX_HOME/auth.json"
PROJ="$SCR/proj"
mkdir -p "$PROJ"
printf '[projects."%s"]\ntrust_level = "trusted"\n' "$PROJ" >"$CODEX_HOME/config.toml"
# The pane's shell is a plain sh that keeps this CODEX_HOME; the person's zsh setup would route
# it to another profile on every cd.
cat >"$SCR/shell" <<SH
#!/bin/sh
export CODEX_HOME=$CODEX_HOME PS1='\$ '
exec /bin/sh -i
SH
chmod +x "$SCR/shell"
printf '[terminal]\nshell = "%s"\n' "$SCR/shell" >"$PROJ/wheel.toml"
start_daemon --state-file "$SCR/state.json"

w space new "$PROJ" >/dev/null
P="$(w space ls | jget spaces.0.root.id)"
w pane start "$P" terminal >/dev/null

w term send "$P" codex --enter
sleep 8
# Codex reads text and Enter arriving together as a paste, so Enter is its own send.
w term send "$P" "what is 6 times 7? reply with just the number"
sleep 1
w term send "$P" "" --enter
if w term read "$P" --new --until "42" --timeout 90 >/dev/null 2>&1; then ok "Codex answers the first prompt"
else fail "Codex answers the first prompt" "$(tail_screen)"; finish; fi
sleep 5   # the daemon's 2 s sampler learns the thread id

# Terminal -> chat: Codex must exit within the daemon's TUI_EXIT (10 s).
t0="$(now)"
if out="$(w pane view "$P" agent 2>&1)"; then t1="$(now)"
  ok "pane view agent succeeds: $out"
  secs="$(perl -e "printf '%.1f', $t1 - $t0")"
  if perl -e "exit !($t1 - $t0 < 10)"; then ok "Codex left within 10 s (${secs} s)"
  else fail "Codex left within 10 s" "took ${secs} s"; fi
else fail "pane view agent succeeds" "$out"; finish; fi

AGENT="$P"
w agent wait "$AGENT" --timeout 60 >/dev/null 2>&1
conv="$(w agent read "$AGENT" 2>&1)"
if printf %s "$conv" | grep -q "what is 6 times 7" && printf %s "$conv" | grep -v "what is 6 times 7" | grep -q "42"; then
  ok "agent read holds the TUI's prompt and its reply"
else fail "agent read holds the TUI's prompt and its reply" "$conv"; fi

if out="$(w agent send "$AGENT" "what is 8 times 9? reply with just the number" 2>&1)" && printf %s "$out" | grep -v "what is 8 times 9" | grep -q "72"; then
  ok "the chat agent continues the TUI's thread"
else fail "the chat agent continues the TUI's thread" "$out"; fi

# Chat -> terminal: codex resume shows both exchanges.
if out="$(w pane view "$P" terminal 2>&1)"; then ok "pane view terminal succeeds: $out"
else fail "pane view terminal succeeds" "$out"; finish; fi
if w term read "$P" --until "72" --timeout 45 >/dev/null 2>&1; then
  screen="$(w term read "$P" --timeout 0 2>&1)"
  if printf %s "$screen" | grep -q "42"; then ok "codex resume shows both exchanges"
  else fail "codex resume shows both exchanges" "$(printf %s "$screen" | clean | tail -15)"; fi
else fail "codex resume shows the chat's exchange" "$(tail_screen)"; fi

# A switch during a turn is refused.
sleep 8   # codex resume finishes loading
w term send "$P" "count slowly from 1 to 40, one number per line, and say nothing else"
sleep 1
w term send "$P" "" --enter
sleep 3
out="$(w pane view "$P" agent 2>&1)"; code=$?
if [ "$code" != 0 ] && printf %s "$out" | grep -q "in the middle of a turn"; then ok "pane view agent during a turn is refused: $out"
else fail "pane view agent during a turn is refused" "exit $code: $out"; fi
[ "$code" = 0 ] && { w agent wait "$AGENT" --timeout 60 >/dev/null 2>&1; w pane view "$P" terminal >/dev/null 2>&1; sleep 8; }

# After the turn it succeeds: the Stop hook reported idle.
for _ in $(seq 60); do
  out="$(w pane view "$P" agent 2>&1)"; code=$?
  if [ "$code" = 0 ]; then ok "pane view agent after the turn ends succeeds: $out"; break; fi
  if ! printf %s "$out" | grep -q "in the middle of a turn"; then fail "pane view agent after the turn ends succeeds" "exit $code: $out"; break; fi
  sleep 3
done
[ "$code" = 0 ] || fail "pane view agent after the turn ends succeeds" "still refused after 180 s: $out"

finish
