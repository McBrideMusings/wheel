#!/bin/bash
# Check external backends (wheel-pmf2.3.4) against a throwaway wheeld: the clock fixture
# (tools/fixtures/plugins/clock) is run by hand, listening on a unix socket and on a WebSocket, and
# named by an [[external_backend]] in a wheel.toml. wheeld connects to it, never starts it, marks
# its feeds stale when it is killed and reconnects when it is run again. Prints PASS or FAIL per
# check. Uses no GUI, focus, mouse or keyboard. Everything it starts is killed by pid at the end.
#
#   tools/plugin-external-backend.sh   # uses target/debug/wheeld and target/debug/wheel
#   WHEELD=... WHEEL=... tools/plugin-external-backend.sh
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=/private/tmp/claude/wheel/peb
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
PORT=$(free_port)
WSPORT=$(free_port)
FAILS=0
PIDS=()
CLOCK=$T/clock/backend.py

pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }

cleanup() {
  for pid in "${PIDS[@]}" ${UNIX_PID:-} ${WS_PID:-}; do kill "$pid" 2>/dev/null; done
  sleep 0.5
  for pid in "${PIDS[@]}" ${UNIX_PID:-} ${WS_PID:-}; do kill -9 "$pid" 2>/dev/null; done
}
trap cleanup EXIT

rm -rf "$T"
mkdir -p "$T/support" "$T/logs"
cp -R "$ROOT/tools/fixtures/plugins/clock" "$T/clock"
SOCK=$T/clock.sock
# A project with the clock package for each case; its own [backend] must never be started.
for p in unix ws notoken badtoken machine; do
  mkdir -p "$T/$p/.wheel/plugins"
  cp -R "$ROOT/tools/fixtures/plugins/clock" "$T/$p/.wheel/plugins/clock"
done
# The machine's entry points nowhere; a project entry with the same id replaces it.
printf '[[external_backend]]\nid = "clock"\nconnect = "unix://%s/nowhere.sock"\n' "$T" > "$T/machine.toml"
printf '[[external_backend]]\nid = "clock"\nconnect = "unix://%s"\n\n[[external_backend]]\nid = "tick"\nconnect = "unix://%s"\n' \
  "$SOCK" "$SOCK" > "$T/unix/wheel.toml"
printf '[[external_backend]]\nid = "clock"\nconnect = "ws://127.0.0.1:%s/clock"\ntoken = "s3cret"\n' "$WSPORT" > "$T/ws/wheel.toml"
printf '[[external_backend]]\nid = "clock"\nconnect = "ws://127.0.0.1:%s/clock"\n' "$WSPORT" > "$T/notoken/wheel.toml"
printf '[[external_backend]]\nid = "clock"\nconnect = "ws://127.0.0.1:%s/clock"\ntoken = "wrong"\n' "$WSPORT" > "$T/badtoken/wheel.toml"
# probe: a pane that may read any feed, to read tick.*, which no package of the project serves.
mkdir -p "$T/unix/.wheel/plugins/probe"
printf '[plugin]\nid = "probe"\nversion = "0.1.0"\n\n[[pane]]\nid = "main"\ntitle = "Probe"\nscript = "main.luau"\nreads = ["*"]\n' \
  > "$T/unix/.wheel/plugins/probe/wheel-plugin.toml"
echo 'return {}' > "$T/unix/.wheel/plugins/probe/main.luau"

"$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
  --log-dir "$T/logs" --cli "$WHEEL" > "$T/wheeld.out" 2>&1 &
WHEELD_PID=$!
PIDS+=($WHEELD_PID)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }

poll() {
  local n=$(( $1 * 10 ))
  shift
  for _ in $(seq 1 "$n"); do
    "$@" > /dev/null 2>&1 && return 0
    sleep 0.1
  done
  return 1
}
make_pane() {
  SPACE=$(w space new "$1")
  PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
  w pane kind "$PANE" plugin --plugin "${2:-clock}" --plugin-pane main > /dev/null
}
start_unix() { python3 "$CLOCK" --listen "unix:$SOCK" >> "$T/unix-backend.log" 2>&1 & UNIX_PID=$!; }
start_ws() { python3 "$CLOCK" --listen "ws:$WSPORT" --token s3cret >> "$T/ws-backend.log" 2>&1 & WS_PID=$!; }
# Succeeds when a read of $2 in pane $1 gets a value with a tick.
ticks() { w plugin feed "$1" "$2" --count 2 2>/dev/null | grep -q '"tick"'; }
ls_has() { w plugin ls --space "$1" 2>&1 | grep -q -- "$2"; }
lines_at_least() { [ "$(wc -l < "$1")" -ge "$2" ]; }
stale_then_fresh() {
  python3 - "$1" <<'PYEOF'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip().startswith("{")]
first = next((i for i, r in enumerate(rows) if r.get("stale")), None)
sys.exit(0 if first is not None and any(r.get("value") and not r.get("stale") for r in rows[first + 1:]) else 1)
PYEOF
}

make_pane "$T/unix"; SU=$SPACE; PU=$PANE
make_pane "$T/unix" probe; PROBE=$PANE
make_pane "$T/ws"; SW=$SPACE; PW=$PANE
make_pane "$T/notoken"; SN=$SPACE; PN=$PANE
make_pane "$T/badtoken"; SB=$SPACE; PB=$PANE
make_pane "$T/machine"; SM=$SPACE; PM=$PANE
echo "unix $SU/$PU  ws $SW/$PW  notoken $SN/$PN  badtoken $SB/$PB  machine $SM/$PM"

# 1 nothing listens yet: a read is refused with the address, and wheeld starts nothing.
w plugin feed "$PU" clock.now --count 1 > /dev/null 2> "$T/down.err"
grep -q "could not connect to unix://$SOCK" "$T/down.err" \
  && pass "a read while nothing listens is refused with the address" \
  || fail "a read while nothing listens is refused with the address: $(cat "$T/down.err")"
ls_has "$SU" "backend: exited (external) unix://$SOCK" \
  && pass "plugin ls shows the external backend in place of the package's own" \
  || fail "plugin ls shows the external backend: $(w plugin ls --space "$SU" 2>&1 | grep -A3 '^clock')"

# 2 run by hand: wheeld connects on its next try and the pane reads the clock.
start_unix
poll 20 ticks "$PU" clock.now \
  && pass "the clock run by hand and registered as external feeds the pane" \
  || fail "the clock run by hand feeds the pane: $(w plugin feed "$PU" clock.now --count 1 2>&1 | head -2)"
ls_has "$SU" "backend: running (external) unix://$SOCK" && ls_has "$SU" "feeds: now, lines" \
  && pass "plugin ls shows it running with the feeds it named" \
  || fail "plugin ls shows it running: $(w plugin ls --space "$SU" 2>&1 | grep -A4 '^clock')"
w plugin action "$PU" clock.echo --args '{"n": 3}' 2> "$T/echo.err" | grep -q '"n":3' \
  && pass "an action reaches the external backend" || fail "an action reaches the external backend: $(cat "$T/echo.err")"
grep -q 'clock backend up' "$T/logs/plugins/clock.log" 2>/dev/null \
  && fail "wheeld never starts the package's own backend: its log has the backend's output" \
  || pass "wheeld never starts the package's own backend"
grep -q "connecting to unix://$SOCK" "$T/logs/plugins/clock.log" \
  && pass "the plugin log records each connection attempt" || fail "the plugin log records the connection: $(tail -3 "$T/logs/plugins/clock.log")"

# 3 killed and run again by hand: the pane goes stale, then recovers.
w plugin feed "$PU" clock.now --count 14 > "$T/recover.jsonl" 2>&1 &
READER=$!
poll 10 lines_at_least "$T/recover.jsonl" 3
kill "$UNIX_PID"; wait "$UNIX_PID" 2>/dev/null
poll 10 ls_has "$SU" "reconnecting in" \
  && pass "a lost connection is retried, and plugin ls says when" \
  || fail "a lost connection is retried: $(w plugin ls --space "$SU" 2>&1 | grep error)"
sleep 2
start_unix
for _ in $(seq 1 200); do kill -0 "$READER" 2>/dev/null || break; sleep 0.1; done
kill "$READER" 2>/dev/null
stale_then_fresh "$T/recover.jsonl" \
  && pass "stopping and restarting it by hand: the pane goes stale then recovers" \
  || fail "the pane goes stale then recovers: $(cut -c1-90 "$T/recover.jsonl" | tail -6)"

# 4 a restart disconnects and connects again; the backend is never told to exit.
w plugin restart clock --space "$SU" > "$T/restart.out" 2>&1 \
  && kill -0 "$UNIX_PID" 2>/dev/null && [ "$(grep -c '^connected' "$T/unix-backend.log")" -ge 3 ] \
  && pass "plugin restart reconnects and leaves the backend running" \
  || fail "plugin restart reconnects: $(cat "$T/restart.out"); $(tail -4 "$T/unix-backend.log")"

# 5 an entry with no package of its id: another pane reads its feeds by their id.
poll 10 ticks "$PROBE" tick.now \
  && pass "an external backend needs no package: tick.now reaches a pane that reads it" \
  || fail "tick.now reaches the probe: $(w plugin feed "$PROBE" tick.now --count 1 2>&1 | head -2)"
w plugin ls --space "$SU" 2>&1 | grep -q "^tick	.*$T/unix/wheel.toml" \
  && pass "plugin ls lists it by its wheel.toml" || fail "plugin ls lists tick: $(w plugin ls --space "$SU" 2>&1 | grep -A2 tick)"

# 6 ws with its token.
start_ws
poll 20 ticks "$PW" clock.now && grep -q 'accepted a WebSocket for /clock' "$T/ws-backend.log" \
  && pass "a ws backend with its token feeds the pane" \
  || fail "a ws backend feeds the pane: $(w plugin feed "$PW" clock.now --count 1 2>&1 | head -2); $(tail -2 "$T/ws-backend.log")"

# 7 ws without a token: refused before any connection.
w plugin feed "$PN" clock.now --count 1 > /dev/null 2> "$T/notoken.err"
grep -q "is a ws:// URL and the entry has no token" "$T/notoken.err" \
  && pass "a ws backend without a token is refused" || fail "a ws backend without a token is refused: $(cat "$T/notoken.err")"
ls_has "$SN" "error: refused: connect" \
  && pass "plugin ls says why it is refused" || fail "plugin ls says why: $(w plugin ls --space "$SN" 2>&1 | grep -A3 '^clock')"

# 8 ws with the wrong token: the backend answers 401 and wheeld says so.
w plugin feed "$PB" clock.now --count 1 > /dev/null 2> "$T/badtoken.err"
grep -q "401" "$T/badtoken.err" && grep -q "refused a WebSocket with authorization 'Bearer wrong'" "$T/ws-backend.log" \
  && pass "a wrong token is sent as Bearer and its 401 reaches the reader" \
  || fail "a wrong token: $(cat "$T/badtoken.err"); $(grep refused "$T/ws-backend.log")"

# 8a the token fixed in the file: the next read connects with it, and the connection with the
# wrong one is not tried again.
printf '[[external_backend]]\nid = "clock"\nconnect = "ws://127.0.0.1:%s/clock"\ntoken = "s3cret"\n' "$WSPORT" > "$T/badtoken/wheel.toml"
poll 10 ticks "$PB" clock.now \
  && pass "a token fixed in wheel.toml is used at the next read, with no restart" \
  || fail "a fixed token is used: $(w plugin feed "$PB" clock.now --count 1 2>&1 | head -2)"
poll 40 grep -q 'not restarted: the \[\[external_backend\]\] clock changed' "$T/logs/plugins/clock.log" \
  && pass "the connection with the old token stops retrying, and the log says why" \
  || fail "the old connection stops retrying: $(grep -E 'not restarted|reconnecting' "$T/logs/plugins/clock.log" | tail -3)"

# 9 the machine's entry holds where no project entry replaces it.
w plugin feed "$PM" clock.now --count 1 > /dev/null 2> "$T/machine.err"
grep -q "could not connect to unix://$T/nowhere.sock" "$T/machine.err" \
  && pass "a space with no entry of its own uses the machine's" || fail "the machine's entry: $(cat "$T/machine.err")"

# 10 wheeld stopping leaves the external backends running.
kill "$WHEELD_PID"; wait "$WHEELD_PID" 2>/dev/null
sleep 0.5
kill -0 "$UNIX_PID" 2>/dev/null && kill -0 "$WS_PID" 2>/dev/null \
  && pass "wheeld stopping leaves the external backends running" \
  || fail "wheeld stopping leaves the external backends running"

echo
if [ "$FAILS" -eq 0 ]; then echo "all checks passed"; else echo "$FAILS check(s) failed"; fi
exit "$FAILS"
