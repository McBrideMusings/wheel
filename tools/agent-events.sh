#!/bin/bash
# Check `wheel agent events --follow` (wheel-2aln.7) against a throwaway wheeld running the fake
# agent: with no pane, the command prints an agent's turn_start and turn_end as one JSON line
# each, a quiet stretch does not end it, it refuses to run without --follow, and it exits with an
# error when wheeld goes away. Prints PASS or FAIL per check. Uses no GUI, focus, mouse or
# keyboard. Everything it starts is killed by pid at the end.
#
#   tools/agent-events.sh        # needs cargo build -p wheeld -p wheel-cli --bins
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
FAKE=${FAKE:-$ROOT/target/debug/wheel-fake-agent}
T=/private/tmp/claude/wheel/agent-events
PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
FAILS=0
PIDS=()

pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }

cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null; done
  sleep 0.5
  for pid in "${PIDS[@]}"; do kill -9 "$pid" 2>/dev/null; done
}
trap cleanup EXIT

for f in "$WHEELD" "$WHEEL" "$FAKE"; do
  [ -x "$f" ] || { echo "missing $f: run cargo build -p wheeld -p wheel-cli --bins" >&2; exit 2; }
done

rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/proj"
printf '[agent.claude]\ncommand = "%s"\n' "$FAKE" > "$T/machine.toml"
SHELL=/bin/sh "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
  --log-dir "$T/logs" --cli "$WHEEL" > "$T/wheeld.out" 2>&1 &
WHEELD_PID=$!
PIDS+=("$WHEELD_PID")
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { env -u WHEEL_PANE -u WHEEL_SPACE "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }

# Waits up to $1 seconds for file $2 to hold a line with "event":"$3".
has_event() {
  for _ in $(seq 1 $(( $1 * 10 ))); do
    grep -q "\"event\":\"$3\"" "$2" && return 0
    sleep 0.1
  done
  return 1
}

if w agent events > "$T/plain.txt" 2>&1; then
  fail "agent events without --follow is refused"
else
  grep -q -- "--follow" "$T/plain.txt" && pass "agent events without --follow is refused and names --follow" \
    || fail "agent events without --follow names --follow: $(cat "$T/plain.txt")"
fi

w agent events --follow > "$T/events.jsonl" 2> "$T/events.err" &
FOLLOW_PID=$!
PIDS+=("$FOLLOW_PID")
sleep 1

SPACE=$(w space new "$T/proj")
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
w pane start "$PANE" agent --agent claude --message hello > "$T/start.txt" 2>&1
has_event 10 "$T/events.jsonl" turn_start && pass "turn_start printed" || fail "turn_start printed: $(cat "$T/events.jsonl")"
has_event 10 "$T/events.jsonl" turn_end && pass "turn_end printed" || fail "turn_end printed: $(cat "$T/events.jsonl")"
python3 - "$T/events.jsonl" "$PANE" "$SPACE" <<'PYEOF' && pass "each line is one hook JSON event naming its pane and space" || fail "line shape: $(cat "$T/events.jsonl")"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
assert rows, "no lines"
for row in rows:
    assert {"event", "agent", "text", "at_ms"} <= row.keys(), row
    assert row.get("pane") == sys.argv[2] and row.get("space") == sys.argv[3], row
PYEOF

# A quiet stretch leaves the stream open, and the next turn still arrives.
sleep 20
kill -0 "$FOLLOW_PID" 2>/dev/null && pass "the stream stays open through 20 s with no events" || fail "the stream ended while quiet: $(cat "$T/events.err")"
w agent send "$PANE" "again" > /dev/null 2>&1
[ "$(grep -c '"event":"turn_end"' "$T/events.jsonl")" -ge 2 ] && pass "a turn after the quiet stretch is printed" \
  || fail "a turn after the quiet stretch is printed: $(cat "$T/events.jsonl")"

kill "$WHEELD_PID"
for _ in $(seq 1 50); do kill -0 "$FOLLOW_PID" 2>/dev/null || break; sleep 0.1; done
if kill -0 "$FOLLOW_PID" 2>/dev/null; then
  fail "the command ends when wheeld goes away"
else
  wait "$FOLLOW_PID"; code=$?
  [ "$code" -ne 0 ] && pass "the command exits $code when wheeld goes away" || fail "the command exited 0 when wheeld went away"
fi

echo "$FAILS failed"
[ "$FAILS" -eq 0 ]
