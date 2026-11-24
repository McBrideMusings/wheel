#!/usr/bin/env bash
# Agent homes from the login shell: agents started in one checkout ask the shell once, and a
# changed startup file asks it again (wheel-qdrj.1).
source "$(dirname "$0")/lib.sh"

PROJ="$SCR/proj"
mkdir -p "$PROJ"
RUNS="$SCR/home-asks"
# The daemon's login shell: counts every run that asks for an agent home, and sets one.
FAKE_SHELL="$SCR/shell"
printf '#!/bin/sh\ncase "$2" in *CLAUDE_CONFIG_DIR*) echo x >>"%s";; esac\nexport CLAUDE_CONFIG_DIR="%s/claude-home"\neval "$2"\n' "$RUNS" "$SCR" >"$FAKE_SHELL"
chmod +x "$FAKE_SHELL"
asks() { [ -f "$RUNS" ] && wc -l <"$RUNS" | tr -d ' ' || echo 0; }

use_fake_agent
DAEMON_SHELL="$FAKE_SHELL" start_daemon

S="$(w space new "$PROJ")"
P1="$(w space ls --json | plutil -extract 0.panes.0 raw -o - -- -)"
P2="$(w pane split "$P1" right)"
P3="$(w pane split "$P2" down)"
starts=()
for p in "$P1" "$P2" "$P3"; do w pane start "$p" agent --message hi >/dev/null & starts+=($!); done
# Only the starts: a bare wait would also wait for the daemon.
wait "${starts[@]}"

[ "$(asks)" = 1 ] && ok "three agents in one checkout ran the login shell once" \
  || fail "three agents in one checkout ran the login shell $(asks) times"
ran="$(grep -c 'agent home from the login shell.*shell_ran=true' "$(LOG)")"
kept="$(grep -c 'agent home from the login shell.*shell_ran=false' "$(LOG)")"
[ "$ran" = 1 ] && [ "$kept" = 2 ] && ok "log: 1 shell_ran=true, 2 shell_ran=false" \
  || fail "log: $ran shell_ran=true, $kept shell_ran=false"
grep -q "home=$SCR/claude-home" "$(LOG)" && ok "the agents got the shell's CLAUDE_CONFIG_DIR" \
  || fail "no agent got $SCR/claude-home"

sleep 1
touch "$PROJ/.envrc"
P4="$(w pane split "$P3" right)"
w pane start "$P4" agent --message hi >/dev/null
[ "$(asks)" = 2 ] && ok "a changed .envrc asked the login shell again" \
  || fail "after a changed .envrc the shell ran $(asks) times in all, want 2"

echo "$PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
