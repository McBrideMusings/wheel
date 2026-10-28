#!/usr/bin/env bash
# § 3 Agent panes after a restart whose login shell is slow: wheeld restores the agent panes with
# their sessions unstarted (the program is not on the bare PATH yet), the Mac app's attach is
# refused with "no session", and once the shell's PATH arrives wheeld starts the sessions. The app
# must attach again by itself. Drives the real Mac app against its own daemon; the slow shell is a
# script that outlives wheeld's 4 s probe the first time it is asked.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

# Phase 1: a daemon that finds the fake agent, one space with one agent pane, saved to disk.
use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
mkdir -p "$SCR/proj"
SP="$(w space new "$SCR/proj")"
PANE="$(w space ls --json | jq -r --arg s "$SP" '.[] | select(.id==$s) | .panes[0]')"
w pane start "$PANE" agent >/dev/null
SESSION="$(w space ls | jq -r '[.. | objects | select(.session?) | .session] | first')"
check "the agent pane has a session" test -n "$SESSION" -a "$SESSION" != null
stop_daemon

# Phase 2: the same state, the agent named without a path, a bare PATH, and a login shell that
# takes 6 s the first time and answers at once the second time.
AGENT_DIR="$(dirname "$FAKE_AGENT")"
printf '[agent.claude]\ncommand = "wheel-fake-agent"\n' >"$INST/wheel.toml"
cat >"$SCR/slowsh" <<EOF
#!/bin/sh
n=\$(cat "$SCR/slowsh.count" 2>/dev/null || echo 0)
echo \$((n + 1)) >"$SCR/slowsh.count"
[ "\$n" = 0 ] && sleep 6
printf '\036%s\036' "$AGENT_DIR:/usr/bin:/bin"
EOF
chmod +x "$SCR/slowsh"
PORT_NOW="$PORT"
env PATH=/usr/bin:/bin SHELL="$SCR/slowsh" "$WHEELD" --listen "127.0.0.1:$PORT_NOW" --support-dir "$INST" --machine-file "$INST/wheel.toml" --log-dir "$LOGS" \
  --state-file "$SCR/spaces.json" --path-from-login-shell >>"$SCR/wheeld.out" 2>&1 &
DPID=$!
wait_port
ok "wheeld is back on the same state with a slow login shell"

prefs_reset
launch_app
session_error() { st --arg s "$SESSION" '[.. | objects | select(.session? == $s)] | first | .error'; }
check "while the shell is slow the app's attach is refused" wait_st "[.. | objects | select(.session? == \"$SESSION\")] | first | .error == \"no session $SESSION\"" 10
check "wheeld starts the session once PATH arrives" poll 40 sh -c "grep -q 'agent started now that its program is found' '$LOGS/wheeld.log'"
check "the app attaches again by itself and the error goes" wait_st "[.. | objects | select(.session? == \"$SESSION\")] | first | .error == null" 10
check "and the app log says why" grep -q "session $SESSION is there now; attaching again" "$(st .log_path)"
finish
