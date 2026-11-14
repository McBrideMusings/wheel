#!/bin/bash
# Check plugin backends and feeds (wheel-pmf2.3.1) against a throwaway wheeld: the clock fixture
# (tools/fixtures/plugins/clock) runs as a backend over stdio and over a unix socket, and the
# `wheel plugin feed` CLI reads its feeds as a plugin pane does. Prints PASS or FAIL per check.
# Uses no GUI, focus, mouse or keyboard. Everything it starts is killed by pid at the end.
#
#   tools/plugin-backend.sh            # uses target/debug/wheeld and target/debug/wheel
#   WHEELD=... WHEEL=... tools/plugin-backend.sh
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=/private/tmp/claude/wheel/pb
PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
FAILS=0
PIDS=()

pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }
check() { if [ "$2" = ok ]; then pass "$1"; else fail "$1 ($2)"; fi; }

cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null; done
  sleep 0.5
  for pid in "${PIDS[@]}"; do kill -9 "$pid" 2>/dev/null; done
  pkill -f "$T/.*backend.py" 2>/dev/null
}
trap cleanup EXIT

rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/proj/.wheel/plugins" "$T/proj2/.wheel/plugins"
: > "$T/machine.toml"
cp -R "$ROOT/tools/fixtures/plugins/clock" "$T/proj/.wheel/plugins/clock"
cp -R "$ROOT/tools/fixtures/plugins/clock" "$T/proj2/.wheel/plugins/clock"
sed -i '' 's/transport = "stdio"/transport = "unix"/' "$T/proj2/.wheel/plugins/clock/wheel-plugin.toml"

"$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
  --log-dir "$T/logs" > "$T/wheeld.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }

# A space for a project and a plugin pane in it: sets SPACE and PANE.
make_pane() {
  SPACE=$(w space new "$1")
  PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
  w pane kind "$PANE" plugin --plugin clock --plugin-pane main > /dev/null
}

make_pane "$T/proj"
SPACE1=$SPACE
PANE1=$PANE
make_pane "$T/proj2"
SPACE2=$SPACE
PANE2=$PANE
PANE=$PANE1
echo "stdio: space $SPACE1 pane $PANE1; unix: space $SPACE2 pane $PANE2"

# 7 before any feed is read: the backend is declared and stopped.
w plugin ls --space "$SPACE1" > "$T/ls-before.txt" 2>&1
grep -q 'backend: stopped' "$T/ls-before.txt" && pass "plugin ls shows the backend stopped before a feed is read" \
  || fail "plugin ls shows the backend stopped before a feed is read: $(cat "$T/ls-before.txt")"

# 1 clock.now: about once a second, tick increasing.
w plugin feed "$PANE" clock.now --count 4 > "$T/now.jsonl" 2>"$T/now.err"
python3 - "$T/now.jsonl" <<'EOF' > "$T/now.verdict"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
ticks = [r["value"]["tick"] for r in rows if r.get("value")]
# The first message is the value now: null before the backend has sent one.
real = ticks
ok = len(real) >= 3 and real == sorted(set(real)) and all(b - a == 1 for a, b in zip(real, real[1:]))
print("ok" if ok else "ticks=%r rows=%d" % (ticks, len(rows)))
EOF
check "clock.now ticks arrive once a second with increasing tick" "$(cat "$T/now.verdict")"

# 2 clock.lines: a value first, then patches only.
w plugin feed "$PANE" clock.lines --count 5 > "$T/lines.jsonl" 2>"$T/lines.err"
python3 - "$T/lines.jsonl" <<'EOF' > "$T/lines.verdict"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
# The reply carries the value now (null before the backend has sent one); then comes the value.
rows = rows[1:] if rows and rows[0].get("value") is None else rows
first, rest = rows[0], rows[1:]
ok = (first.get("value") is not None and "patch" not in first and len(rest) >= 2
      and all(r.get("patch") and "value" not in r for r in rest))
print("ok" if ok else "rows=%r" % rows)
EOF
check "clock.lines is a value then patches, never a full value again" "$(cat "$T/lines.verdict")"

# 3 the log.
w plugin logs clock --space "$SPACE1" > "$T/logs.txt" 2>&1
grep -q 'clock backend up' "$T/logs.txt" && grep -qE '^[0-9]{2}:[0-9]{2}:[0-9]{2} UTC info initialized for clock' "$T/logs.txt" \
  && pass "plugin logs prints the backend's stderr and its log messages" \
  || fail "plugin logs prints the backend's output: $(cat "$T/logs.txt")"

# 4 wheel.spaces lists the spaces.
w plugin feed "$PANE" wheel.spaces --count 1 > "$T/spaces.jsonl" 2>&1
python3 - "$T/spaces.jsonl" "$T/proj" <<'EOF' > "$T/spaces.verdict"
import json, os, sys
row = json.loads(open(sys.argv[1]).readline())
cwds = [os.path.realpath(s["cwd"]) for s in row["value"]]
print("ok" if os.path.realpath(sys.argv[2]) in cwds and len(cwds) == 2 else "value=%r" % row)
EOF
check "wheel.spaces lists both spaces" "$(cat "$T/spaces.verdict")"

# 4a a new space reaches a reader of wheel.spaces as a changed value.
mkdir -p "$T/proj3"
w plugin feed "$PANE" wheel.spaces --count 2 > "$T/spaces2.jsonl" 2>&1 &
READER=$!
sleep 1
w space new "$T/proj3" > /dev/null
wait "$READER"
python3 - "$T/spaces2.jsonl" <<'EOF2' > "$T/spaces2.verdict"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
print("ok" if [len(r["value"]) for r in rows] == [2, 3] else "rows=%r" % rows)
EOF2
check "a new space is published to a wheel.spaces reader" "$(cat "$T/spaces2.verdict")"

# 4b wheel.panes and wheel.agents answer.
w plugin feed "$PANE" wheel.panes --count 1 | grep -q '"kind":"plugin"' && pass "wheel.panes lists the plugin panes" \
  || fail "wheel.panes lists the plugin panes"
w plugin feed "$PANE" wheel.agents --count 1 | grep -q '"value":\[\]' && pass "wheel.agents is empty with no agent running" \
  || fail "wheel.agents is empty with no agent running"

# 4c a feed the backend does not have, and a plugin that does not exist.
w plugin feed "$PANE" clock.nope --count 1 > /dev/null 2> "$T/nofeed.err"
grep -q 'no feed nope' "$T/nofeed.err" && pass "a feed the backend lacks is refused with a clear error" \
  || fail "a feed the backend lacks is refused: $(cat "$T/nofeed.err")"

# 7 plugin ls with the backend running.
w plugin ls --space "$SPACE1" > "$T/ls-after.txt" 2>&1
PID=$(sed -n 's/.*backend: running pid \([0-9]*\).*/\1/p' "$T/ls-after.txt" | head -1)
if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then pass "plugin ls shows the backend running with a live pid ($PID)"
else fail "plugin ls shows the backend running with a pid: $(cat "$T/ls-after.txt")"; fi

# 5 after the subscriber closed, the backend was told.
sleep 1
grep -q 'unsubscribed now' "$T/logs/plugins/clock.log" && pass "closing the subscriber unsubscribes the backend feed (stdio)" \
  || fail "closing the subscriber unsubscribes the backend feed (stdio): $(tail -5 "$T/logs/plugins/clock.log")"

# 5b a subscriber killed without unsubscribing (connection closed) also unsubscribes.
( w plugin feed "$PANE" clock.lines > /dev/null 2>&1 & echo $! > "$T/feed.pid"; wait ) &
sleep 3
kill "$(cat "$T/feed.pid")" 2>/dev/null
sleep 2
grep -q 'unsubscribed lines' "$T/logs/plugins/clock.log" && pass "a closed connection unsubscribes the backend feed" \
  || fail "a closed connection unsubscribes the backend feed"

# 6 the same over a unix socket (a second package in the second project, same plugin id, own folder).
w plugin feed "$PANE2" clock.now --count 3 > "$T/now-unix.jsonl" 2>"$T/now-unix.err"
python3 - "$T/now-unix.jsonl" <<'EOF' > "$T/now-unix.verdict"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
ticks = [r["value"]["tick"] for r in rows if r.get("value")]
print("ok" if len(ticks) >= 2 and ticks == sorted(set(ticks)) else "rows=%r" % rows)
EOF
check "clock.now over a unix socket ticks" "$(cat "$T/now-unix.verdict")"
w plugin ls --space "$SPACE2" 2>&1 | grep -q 'backend: running pid [0-9]* (unix)' \
  && pass "plugin ls shows the unix backend running" || fail "plugin ls shows the unix backend running"
sleep 1
grep -q 'unsubscribed now' "$T/logs/plugins/clock.log" && [ "$(grep -c 'unsubscribed now' "$T/logs/plugins/clock.log")" -ge 2 ] \
  && pass "closing the subscriber unsubscribes the backend feed (unix)" \
  || fail "closing the subscriber unsubscribes the backend feed (unix)"

# A backend whose process ends is marked exited with its status.
BACKEND_PID=$(sed -n 's/.*backend: running pid \([0-9]*\) (stdio).*/\1/p' "$T/ls-after.txt" | head -1)
if [ -n "$BACKEND_PID" ]; then
  kill -9 "$BACKEND_PID" 2>/dev/null
  sleep 1
  w plugin ls --space "$SPACE1" 2>&1 | grep -q 'error: .*ended by signal 9\|error: .*exited with status' \
    && pass "a backend that dies is marked exited with how it ended" \
    || fail "a backend that dies is marked exited: $(w plugin ls --space "$SPACE1" 2>&1)"
fi

# A backend that dies while a pane still reads a feed: the next feed_subscribe starts a fresh
# process, which is sent feed/subscribe for the feed the first reader still holds, so both get ticks.
w plugin feed "$PANE" clock.now --count 9 > "$T/restart-a.jsonl" 2>&1 &
READER_A=$!
sleep 3
NEWPID=$(w plugin ls --space "$SPACE1" 2>&1 | sed -n 's/.*backend: running pid \([0-9]*\) (stdio).*/\1/p' | head -1)
if [ -n "$NEWPID" ]; then
  kill -9 -- "-$NEWPID" 2>/dev/null
  sleep 1
  w plugin feed "$PANE" clock.now --count 3 > "$T/restart-b.jsonl" 2>&1
  wait "$READER_A"
  python3 - "$T/restart-a.jsonl" "$T/restart-b.jsonl" <<'EOF3' > "$T/restart.verdict"
import json, sys
def ticks(path):
    rows = [json.loads(l) for l in open(path) if l.strip().startswith("{")]
    return [r["value"]["tick"] for r in rows if r.get("value")]
a, b = ticks(sys.argv[1]), ticks(sys.argv[2])
reset = next((i for i in range(1, len(a)) if a[i] <= a[i - 1]), None)
ok = len(b) >= 2 and reset is not None and len(a) - reset >= 2
print("ok" if ok else "a=%r b=%r" % (a, b))
EOF3
  check "a restarted backend is sent the feeds an older reader still holds" "$(cat "$T/restart.verdict")"
else
  fail "a restarted backend is sent the feeds an older reader still holds: no running backend pid"
  kill "$READER_A" 2>/dev/null
fi

echo
if [ "$FAILS" -eq 0 ]; then echo "all checks passed"; else echo "$FAILS check(s) failed"; fi
exit "$FAILS"
