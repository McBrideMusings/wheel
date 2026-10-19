#!/usr/bin/env bash
# § 2 The daemon at its socket: terminals, replay, refused tokens, signals, log lines.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

start_daemon --no-lead

# alive_of <term>: "true" or "false" from `status`, "absent" when status lists no such terminal,
# "error" when status itself failed.
alive_of() {
  local s
  s="$(w status 2>/dev/null | plutil -convert json -o - - 2>/dev/null | tr -d ' \n')" || { echo error; return; }
  [ -n "$s" ] || { echo error; return; }
  printf %s "$s" | grep -o "\"id\":\"$1\"[^}]*" | grep -o '"alive":[a-z]*' | cut -d: -f2 | grep . || echo absent
}

T="$(w term open --cwd /tmp)"
eq "term open prints a terminal id" "$T" "t1"
has_log "pty spawned term=t1 pid=Some\([0-9]+\).*owner=Client" "log: pty spawned owner=Client"

w term send "$T" 'echo wheel-$((40+2))' --enter
expect_exit 0 "term read --until sees the arithmetic marker, not the echoed input" \
  w term read "$T" --until 'wheel-42' --timeout 10
has_log "attach .*term=t1 replay_bytes=[1-9]" "log: attach with replay_bytes > 0"

# A new connection (a fresh `term read`) is attached to the same terminal: the replay carries the marker.
out="$(w term read "$T" --timeout 0.5 | tr -d '\033' )"
if printf %s "$out" | grep -q 'wheel-42'; then ok "reattach replays output printed before the disconnect"; else fail "reattach replays the marker" "$out"; fi

eq "status lists the terminal alive" "$(alive_of t1)" "true"
eq "status reports log_path" "$(w status | jget log_path)" "$(LOG)"

# A wrong token is refused.
echo wrong >"$SCR/badtoken"
expect_exit_out 5 '401' "a wrong token is refused" \
  "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$SCR/badtoken" status
has_log "refused: missing or wrong token" "log: refused: missing or wrong token"

# Close.
expect_exit 0 "term close exits 0" w term close "$T"
eq "closed terminal leaves status" "$(alive_of t1)" "absent"
expect_exit_out 6 "no terminal t1" "closing again exits 6" w term close "$T"
has_log "term closed term=t1" "log: term closed"
expect_exit_out 6 "no terminal t-missing" "term send to a missing terminal exits 6" w term send t-missing hi

# A shell that exits with nobody attached stays in status dead; one read sees the exit (4), then it is gone (6).
T2="$(w term open --cwd /tmp)"
w term send "$T2" 'exit 7' --enter
dead() { [ "$(alive_of "$T2")" = false ]; }
if poll 20 dead; then ok "an exited shell stays in status with alive: false"; else fail "exited shell shows alive: false" "alive=$(alive_of "$T2")"; fi
expect_exit 4 "term read --until on an exited terminal exits 4" w term read "$T2" --until nomatch-zz --timeout 5
eq "after that read the terminal has left status" "$(alive_of "$T2")" "absent"
expect_exit 6 "another read exits 6" w term read "$T2" --until nomatch-zz --timeout 1
has_log "exited terminal dropped" "log: exited terminal dropped"

# SIGTERM: the daemon exits 0 fast, the shell's pid is gone, the log's last line names the signal, an attached read exits 7.
T3="$(w term open --cwd /tmp)"
SHELLPID="$(w status | jget terms.0.pid)"
check "status reports the shell's pid" test -n "$SHELLPID"
bg bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' term read $T3 --timeout 30 >/dev/null 2>&1; echo \$? >'$SCR/readexit'"
READER=$BGPID
sleep 0.5
start_ns=$(perl -MTime::HiRes=time -e 'printf "%d", time*1000')
kill -TERM "$DPID"
wait "$DPID"; code=$?
end_ns=$(perl -MTime::HiRes=time -e 'printf "%d", time*1000')
DPID=""
eq "SIGTERM exits wheeld 0" "$code" "0"
if [ $((end_ns - start_ns)) -lt 1000 ]; then ok "SIGTERM stops wheeld in $((end_ns - start_ns)) ms"; else fail "SIGTERM is quick" "$((end_ns - start_ns)) ms"; fi
wait "$READER" 2>/dev/null
eq "the attached term read exits 7 (lost the connection)" "$(cat "$SCR/readexit" 2>/dev/null)" "7"
if [ -n "$SHELLPID" ] && ! kill -0 "$SHELLPID" 2>/dev/null; then ok "the shell's pid is gone after SIGTERM"; else fail "the shell's pid is gone after SIGTERM" "pid '$SHELLPID'"; fi
eq "log's last line is the stop line" "$(tail -1 "$(LOG)" | sed 's/\x1b\[[0-9;]*m//g' | grep -c 'wheeld stopping signal=SIGTERM')" "1"

# SIGINT on a fresh daemon.
start_daemon --no-lead
w term open --cwd /tmp >/dev/null
start_ns=$(perl -MTime::HiRes=time -e 'printf "%d", time*1000')
kill -INT "$DPID"; wait "$DPID"; code=$?; DPID=""
end_ns=$(perl -MTime::HiRes=time -e 'printf "%d", time*1000')
eq "SIGINT exits wheeld 0" "$code" "0"
if [ $((end_ns - start_ns)) -lt 1000 ]; then ok "SIGINT stops wheeld in $((end_ns - start_ns)) ms"; else fail "SIGINT is quick" "$((end_ns - start_ns)) ms"; fi
eq "log's last line names SIGINT" "$(tail -1 "$(LOG)" | grep -c 'wheeld stopping signal=SIGINT')" "1"

# The fake agent child dies with the daemon.
start_daemon --lead-cmd "$FAKE_AGENT" --lead-cwd /tmp
lead_ready() { [ "$(w status | jget lead.phase)" = ready ]; }
poll 20 lead_ready && ok "the lead (fake agent) is ready" || fail "lead ready" "$(w status | jget lead.phase)"
AGENT_PID="$(pgrep -P "$DPID" -f wheel-fake-agent | head -1)"
check "wheeld has a wheel-fake-agent child" test -n "$AGENT_PID"
kill -TERM "$DPID"; wait "$DPID"; DPID=""
sleep 0.3
if [ -n "$AGENT_PID" ] && ! kill -0 "$AGENT_PID" 2>/dev/null; then ok "the agent child is gone after SIGTERM"; else fail "the agent child is gone after SIGTERM" "pid '$AGENT_PID'"; fi

finish
