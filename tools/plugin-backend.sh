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
mkdir -p "$T/support" "$T/logs" "$T/proj/.wheel/plugins" "$T/proj2/.wheel/plugins" "$T/proj4/.wheel/plugins" "$T/proj5/.wheel/plugins/probe" "$T/proj6/.wheel/plugins" "$T/proj7/.wheel/plugins" "$T/proj8/.wheel/plugins" "$T/proj9/.wheel/plugins" "$T/proj10/.wheel/plugins/quiet" "$T/proj11/.wheel/plugins/crash" "$T/proj12/.wheel/plugins" "$T/proj13/.wheel/plugins"
: > "$T/machine.toml"
cp -R "$ROOT/tools/fixtures/plugins/clock" "$T/proj/.wheel/plugins/clock"
cp -R "$ROOT/tools/fixtures/plugins/clock" "$T/proj2/.wheel/plugins/clock"
cp -R "$ROOT/tools/fixtures/plugins/clock" "$T/proj4/.wheel/plugins/clock"
cp -R "$ROOT/tools/fixtures/plugins/reader" "$T/proj4/.wheel/plugins/reader"
# probe: a pane that may read any feed, to reach what the clock pane's reads exclude.
cp -R "$ROOT/tools/fixtures/plugins/clock" "$T/proj5/.wheel/plugins/clock"
printf '[plugin]\nid = "probe"\nversion = "0.1.0"\n\n[[pane]]\nid = "main"\ntitle = "Probe"\nscript = "main.luau"\nreads = ["*"]\n' \
  > "$T/proj5/.wheel/plugins/probe/wheel-plugin.toml"
echo 'return {}' > "$T/proj5/.wheel/plugins/probe/main.luau"
cp -R "$ROOT/tools/fixtures/plugins/clock" "$T/proj6/.wheel/plugins/clock"
cp -R "$ROOT/tools/fixtures/plugins/clock" "$T/proj7/.wheel/plugins/clock"
cp -R "$ROOT/tools/fixtures/plugins/clock" "$T/proj8/.wheel/plugins/clock"
# proj9: the clock that is never started again; proj10: a backend that exits with status 0.
cp -R "$ROOT/tools/fixtures/plugins/clock" "$T/proj9/.wheel/plugins/clock"
sed -i '' 's/^transport = "stdio"/transport = "stdio"\nrestart = "never"/' "$T/proj9/.wheel/plugins/clock/wheel-plugin.toml"
printf '[plugin]\nid = "quiet"\nversion = "0.1.0"\n\n[backend]\ncommand = "exit 0"\n\n[[pane]]\nid = "main"\ntitle = "Quiet"\nscript = "main.luau"\nreads = ["*"]\n' \
  > "$T/proj10/.wheel/plugins/quiet/wheel-plugin.toml"
echo 'return {}' > "$T/proj10/.wheel/plugins/quiet/main.luau"
sed -i '' 's/transport = "stdio"/transport = "unix"/' "$T/proj2/.wheel/plugins/clock/wheel-plugin.toml"
# proj11: a backend that exits with status 1 at once, so it loops through the backoff.
printf '[plugin]\nid = "crash"\nversion = "0.1.0"\n\n[backend]\ncommand = "exit 1"\n\n[[pane]]\nid = "main"\ntitle = "Crash"\nscript = "main.luau"\nreads = ["*"]\n' \
  > "$T/proj11/.wheel/plugins/crash/wheel-plugin.toml"
echo 'return {}' > "$T/proj11/.wheel/plugins/crash/main.luau"
# proj12: the clock that is never started again. proj13: the clock whose command prints a marker
# to its log and whose watch names every file, so an edit to the manifest is a watched change.
cp -R "$ROOT/tools/fixtures/plugins/clock" "$T/proj12/.wheel/plugins/clock"
sed -i '' 's/^transport = "stdio"/transport = "stdio"\nrestart = "never"/' "$T/proj12/.wheel/plugins/clock/wheel-plugin.toml"
cp -R "$ROOT/tools/fixtures/plugins/clock" "$T/proj13/.wheel/plugins/clock"
sed -i '' -e 's/^command = .*/command = "echo marker-one >\&2; exec python3 backend.py"/' -e 's/^watch = .*/watch = ["*"]/' \
  "$T/proj13/.wheel/plugins/clock/wheel-plugin.toml"

"$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
  --log-dir "$T/logs" > "$T/wheeld.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }

# Runs a command every 0.1 s for up to $1 seconds: succeeds as soon as the command does.
poll() {
  local n=$(( $1 * 10 ))
  shift
  for _ in $(seq 1 "$n"); do
    "$@" > /dev/null 2>&1 && return 0
    sleep 0.1
  done
  return 1
}
# The pid of plugin clock's backend as `plugin ls` reports it in space $1 (empty when none runs).
backend_pid() {
  w plugin ls --json --space "$1" 2>/dev/null | python3 -c '
import json, sys
for p in json.load(sys.stdin):
    if p["id"] == "clock" and p.get("backend", {}).get("pid"):
        print(p["backend"]["pid"])
'
}
# Succeeds when a pid other than $2 is the backend of space $1.
backend_pid_not() {
  local now
  now=$(backend_pid "$1")
  [ -n "$now" ] && [ "$now" != "$2" ] && kill -0 "$now" 2>/dev/null
}
# Succeeds when file $1 holds a feed line with "stale":true and a later line with a value and no stale.
stale_then_fresh() {
  python3 - "$1" <<'PYEOF'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip().startswith("{")]
first = next((i for i, r in enumerate(rows) if r.get("stale")), None)
sys.exit(0 if first is not None and any(r.get("value") and not r.get("stale") for r in rows[first + 1:]) else 1)
PYEOF
}

# Succeeds when `plugin ls` for space $1 holds the text $2.
ls_has() { w plugin ls --space "$1" 2>&1 | grep -q "$2"; }
# Succeeds when some backend of space $1 runs with a pid.
backend_up() { [ -n "$(backend_pid "$1")" ]; }
# Succeeds when the backend of space $1 is down for good: no pid and no restart pending.
stays_down() { [ -z "$(backend_pid "$1")" ] && ! ls_has "$1" 'restarting in'; }
# Succeeds when file $1 holds the line $2 exactly.
has_line() { grep -qx -- "$2" "$1"; }
# Succeeds when file $1 has at least $2 lines.
lines_at_least() { [ "$(wc -l < "$1")" -ge "$2" ]; }


# A space for a project and a plugin pane in it: sets SPACE and PANE.
make_pane() {
  SPACE=$(w space new "$1")
  PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
  w pane kind "$PANE" plugin --plugin "${2:-clock}" --plugin-pane main > /dev/null
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
poll 10 lines_at_least "$T/spaces2.jsonl" 1
w space new "$T/proj3" > /dev/null
wait "$READER"
python3 - "$T/spaces2.jsonl" <<'EOF2' > "$T/spaces2.verdict"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
print("ok" if [len(r["value"]) for r in rows] == [2, 3] else "rows=%r" % rows)
EOF2
check "a new space is published to a wheel.spaces reader" "$(cat "$T/spaces2.verdict")"

make_pane "$T/proj5" probe
PROBE=$PANE
PANE=$PANE1
# 4b wheel.panes and wheel.agents answer.
w plugin feed "$PROBE" wheel.panes --count 1 | grep -q '"kind":"plugin"' && pass "wheel.panes lists the plugin panes" \
  || fail "wheel.panes lists the plugin panes"
w plugin feed "$PROBE" wheel.agents --count 1 | grep -q '"value":\[\]' && pass "wheel.agents is empty with no agent running" \
  || fail "wheel.agents is empty with no agent running"

# 4c a feed the backend does not have, and a plugin that does not exist.
w plugin feed "$PROBE" clock.nope --count 1 > /dev/null 2> "$T/nofeed.err"
grep -q 'no feed nope' "$T/nofeed.err" && pass "a feed the backend lacks is refused with a clear error" \
  || fail "a feed the backend lacks is refused: $(cat "$T/nofeed.err")"

# 8 a listed action runs, and its result carries the args and the caller's pane and plugin.
w plugin action "$PANE" clock.echo --args '{"n": 7}' > "$T/echo.json" 2>"$T/echo.err"
python3 - "$T/echo.json" "$PANE1" "$SPACE1" <<'EOF4' > "$T/echo.verdict"
import json, sys
r = json.loads(open(sys.argv[1]).readline())
want = {"pane": sys.argv[2], "space": sys.argv[3], "plugin": "clock", "plugin_pane": "main"}
print("ok" if r == {"echo": {"n": 7}, "caller": want} else "result=%r" % r)
EOF4
check "a listed action runs and returns its args and caller" "$(cat "$T/echo.verdict")"
cat "$T/echo.json"

# 9 an action the manifest lacks is refused by wheeld, though the backend lists it.
w plugin action "$PANE" clock.hidden > /dev/null 2> "$T/hidden.err"
grep -q 'may not invoke clock.hidden; its actions: clock.echo, clock.fail' "$T/hidden.err" \
  && pass "an action the manifest does not list is refused" \
  || fail "an action the manifest does not list is refused: $(cat "$T/hidden.err")"
cat "$T/hidden.err"
w plugin action "$PANE" wheel.anything > /dev/null 2> "$T/wheelact.err"
grep -q 'wheeld has no actions' "$T/wheelact.err" && pass "a wheel.* action is refused" \
  || fail "a wheel.* action is refused: $(cat "$T/wheelact.err")"

# 10 the backend's own error reaches the caller.
w plugin action "$PANE" clock.fail > /dev/null 2> "$T/fail.err"
grep -q 'the clock refuses' "$T/fail.err" && pass "a backend error reaches the caller as an error" \
  || fail "a backend error reaches the caller as an error: $(cat "$T/fail.err")"
cat "$T/fail.err"

# 11 a feed outside the pane's reads is refused.
w plugin feed "$PANE" clock.secret --count 1 > /dev/null 2> "$T/secret.err"
grep -q 'pane .* (clock.main) may not read clock.secret; its reads: clock.now, clock.lines, wheel.spaces' "$T/secret.err" \
  && pass "a feed outside the pane's reads is refused" \
  || fail "a feed outside the pane's reads is refused: $(cat "$T/secret.err")"
cat "$T/secret.err"
w plugin feed "$PANE" wheel.agents --count 1 > /dev/null 2> "$T/agents.err"
grep -q 'may not read wheel.agents; its reads: clock.now, clock.lines, wheel.spaces' "$T/agents.err" \
  && pass "a wheel.* feed outside the pane's reads is refused" \
  || fail "a wheel.* feed outside the pane's reads is refused: $(cat "$T/agents.err")"

# 12 a pane of another plugin reads clock.now through its glob clock.*.
make_pane "$T/proj4" reader
READER_PANE=$PANE
PANE=$PANE1
w plugin feed "$READER_PANE" clock.now --count 2 > "$T/reader.jsonl" 2>"$T/reader.err"
python3 - "$T/reader.jsonl" <<'EOF5' > "$T/reader.verdict"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
print("ok" if any(r.get("value") for r in rows) else "rows=%r" % rows)
EOF5
check "a pane of the reader plugin reads clock.now through clock.*" "$(cat "$T/reader.verdict")"
w plugin feed "$READER_PANE" wheel.spaces --count 1 > /dev/null 2> "$T/reader2.err"
grep -q 'may not read wheel.spaces; its reads: clock.\*' "$T/reader2.err" \
  && pass "the reader pane may not read what clock.* does not match" \
  || fail "the reader pane may not read what clock.* does not match: $(cat "$T/reader2.err")"

# 7 plugin ls with the backend running.
w plugin ls --space "$SPACE1" > "$T/ls-after.txt" 2>&1
PID=$(sed -n 's/.*backend: running pid \([0-9]*\).*/\1/p' "$T/ls-after.txt" | head -1)
if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then pass "plugin ls shows the backend running with a live pid ($PID)"
else fail "plugin ls shows the backend running with a pid: $(cat "$T/ls-after.txt")"; fi

# 5 after the subscriber closed, the backend was told.
poll 10 grep -q 'unsubscribed now' "$T/logs/plugins/clock.log"
grep -q 'unsubscribed now' "$T/logs/plugins/clock.log" && pass "closing the subscriber unsubscribes the backend feed (stdio)" \
  || fail "closing the subscriber unsubscribes the backend feed (stdio): $(tail -5 "$T/logs/plugins/clock.log")"

# 5b a subscriber killed without unsubscribing (connection closed) also unsubscribes.
( w plugin feed "$PANE" clock.lines > "$T/feed5b.out" 2>&1 & echo $! > "$T/feed.pid"; wait ) &
poll 10 lines_at_least "$T/feed5b.out" 3
kill "$(cat "$T/feed.pid")" 2>/dev/null
poll 10 grep -q 'unsubscribed lines' "$T/logs/plugins/clock.log"
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
poll 10 [ "$(grep -c 'unsubscribed now' "$T/logs/plugins/clock.log")" -ge 2 ]
grep -q 'unsubscribed now' "$T/logs/plugins/clock.log" && [ "$(grep -c 'unsubscribed now' "$T/logs/plugins/clock.log")" -ge 2 ] \
  && pass "closing the subscriber unsubscribes the backend feed (unix)" \
  || fail "closing the subscriber unsubscribes the backend feed (unix)"

# A backend whose process ends is marked exited with its status.
BACKEND_PID=$(sed -n 's/.*backend: running pid \([0-9]*\) (stdio).*/\1/p' "$T/ls-after.txt" | head -1)
if [ -n "$BACKEND_PID" ]; then
  kill -9 "$BACKEND_PID" 2>/dev/null
  # The default policy starts it again after 1 s: the exited state is visible until then.
  if poll 5 ls_has "$SPACE1" "backend: exited"; then
    w plugin ls --space "$SPACE1" > "$T/ls-dead.txt" 2>&1
    pass "a backend that dies is marked exited with how it ended"
  else
    fail "a backend that dies is marked exited: $(w plugin ls --space "$SPACE1" 2>&1)"
  fi
fi

# A backend that dies while a pane still reads a feed: the supervisor starts a fresh process after
# its 1 s wait, and sends it feed/subscribe for the feed the first reader still holds, so both
# that reader and a second one get ticks from the new process. The earlier kill's restart is
# waited out first: a read during the wait is refused.
poll 15 backend_up "$SPACE1"
w plugin feed "$PANE" clock.now --count 9 > "$T/restart-a.jsonl" 2>&1 &
READER_A=$!
poll 10 lines_at_least "$T/restart-a.jsonl" 3
NEWPID=$(w plugin ls --space "$SPACE1" 2>&1 | sed -n 's/.*backend: running pid \([0-9]*\) (stdio).*/\1/p' | head -1)
if [ -n "$NEWPID" ]; then
  kill -9 -- "-$NEWPID" 2>/dev/null
  poll 15 backend_pid_not "$SPACE1" "$NEWPID"
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

# Starts `wheel plugin feed PANE FEED` in the background: sets FEED_PID, output in $T/$1.out and .err.
start_feed() {
  w plugin feed "$2" "$3" > "$T/$1.out" 2> "$T/$1.err" &
  FEED_PID=$!
  PIDS+=($FEED_PID)
  for _ in $(seq 1 50); do
    grep -q '"tick"' "$T/$1.out" 2>/dev/null && return 0
    sleep 0.2
  done
  return 1
}
# Waits up to 10 s for process $1 to end: ok, or "still running".
ended() {
  for _ in $(seq 1 50); do
    kill -0 "$1" 2>/dev/null || { echo ok; return; }
    sleep 0.2
  done
  echo "still running"
}

# 13 a pane that changes kind loses its feed subscription.
make_pane "$T/proj6"
KIND_PANE=$PANE
PANE=$PANE1
if start_feed kind "$KIND_PANE" clock.now; then
  w pane kind "$KIND_PANE" terminal > /dev/null
  check "changing a plugin pane's kind ends its feed subscription" "$(ended "$FEED_PID")"
  grep -q 'is not a plugin pane' "$T/kind.err" \
    && pass "the refusal names the pane as no longer a plugin pane" \
    || fail "the refusal names the pane as no longer a plugin pane: $(cat "$T/kind.err")"
  N=$(wc -l < "$T/kind.out"); sleep 2
  [ "$(wc -l < "$T/kind.out")" -eq "$N" ] && pass "no feed update arrives after the kind change" \
    || fail "no feed update arrives after the kind change"
else
  fail "changing a plugin pane's kind ends its feed subscription: no first tick"
fi

# 14 narrowing the manifest's reads ends a subscription to a feed it no longer lists.
make_pane "$T/proj7"
NARROW_PANE=$PANE
PANE=$PANE1
if start_feed narrow "$NARROW_PANE" clock.now; then
  sed -i '' 's/reads = \["clock.now", "clock.lines", "wheel.spaces"\]/reads = ["clock.lines", "wheel.spaces"]/' \
    "$T/proj7/.wheel/plugins/clock/wheel-plugin.toml"
  check "narrowing the manifest's reads ends a subscription it no longer allows" "$(ended "$FEED_PID")"
  grep -q 'may not read clock.now; its reads: clock.lines, wheel.spaces' "$T/narrow.err" \
    && pass "the refusal names the feed and the narrowed reads" \
    || fail "the refusal names the feed and the narrowed reads: $(cat "$T/narrow.err")"
  cat "$T/narrow.err"
else
  fail "narrowing the manifest's reads ends a subscription it no longer allows: no first tick"
fi

# 15 kill -9 on the backend: a reader is sent its last value stale, then fresh ticks; wheeld and
# the terminals keep running; the backend is a new process.
make_pane "$T/proj8"
S8=$SPACE
P8=$PANE
PANE=$PANE1
TERM8=$(w pane run "$P8" "sleep 60137")
if start_feed stale "$P8" clock.now; then
  poll 5 pgrep -f "sleep 60137"
  SLEEP_PID=$(pgrep -f "sleep 60137" | head -1)
  PID8=$(backend_pid "$S8")
  kill -9 "$PID8"
  check "a killed backend's reader gets a stale value, then a fresh one" \
    "$(poll 15 stale_then_fresh "$T/stale.out" && echo ok || echo "$(cat "$T/stale.out")")"
  grep '"stale":true' "$T/stale.out" | head -1 | grep -q '"value":{' \
    && pass "the stale update carries the last value" || fail "the stale update carries the last value"
  poll 10 backend_pid_not "$S8" "$PID8" && pass "plugin ls shows a new backend pid after the kill ($PID8 to $(backend_pid "$S8"))" \
    || fail "plugin ls shows a new backend pid after the kill"
  kill -0 "${PIDS[0]}" 2>/dev/null && pass "wheeld is still the same process after the backend died" \
    || fail "wheeld is still the same process after the backend died"
  if [ -n "$SLEEP_PID" ] && kill -0 "$SLEEP_PID" 2>/dev/null && w pane ls | grep -q "$TERM8"; then
    pass "a terminal pane made before the kill still exists and its process lives"
  else
    fail "a terminal pane made before the kill still exists and its process lives (pid $SLEEP_PID, pane $TERM8)"
  fi
  grep -q 'restarting in 1 s after: .*signal 9' "$T/logs/plugins/clock.log" \
    && pass "the plugin log says why and when the backend restarts" \
    || fail "the plugin log says why and when the backend restarts"

  # 16 a change to a watched file restarts the backend; a change to another file does not.
  PID8=$(backend_pid "$S8")
  echo "# touched" >> "$T/proj8/.wheel/plugins/clock/backend.py"
  poll 10 backend_pid_not "$S8" "$PID8" && pass "rewriting the watched file restarts the backend with a new pid" \
    || fail "rewriting the watched file restarts the backend with a new pid"
  # The launch after the kill was under 10 s ago, so the change counts as a failure for the backoff.
  grep -qE 'restarting in [0-9]+ s: backend.py changed' "$T/logs/plugins/clock.log" \
    && pass "a watched change within 10 s of the last launch waits the backoff and the log names the file" \
    || fail "a watched change within 10 s of the last launch waits the backoff and the log names the file"
  PID8=$(backend_pid "$S8")
  echo x > "$T/proj8/.wheel/plugins/clock/notes.txt"
  sleep 1.5
  [ "$(backend_pid "$S8")" = "$PID8" ] && pass "a change to a file the watch does not name leaves the backend alone" \
    || fail "a change to a file the watch does not name leaves the backend alone"

  # 17 wheel plugin restart gives a new process, and the reader recovers.
  PID8=$(backend_pid "$S8")
  LINES_BEFORE=$(wc -l < "$T/stale.out")
  OUT=$(w plugin restart clock --space "$S8" 2>&1)
  echo "$OUT"
  NEW8=$(backend_pid "$S8")
  if [ -n "$NEW8" ] && [ "$NEW8" != "$PID8" ] && echo "$OUT" | grep -q "restarted clock (pid $NEW8)"; then
    pass "wheel plugin restart starts a new backend process and prints its pid"
  else
    fail "wheel plugin restart starts a new backend process and prints its pid: $OUT ($PID8 to $NEW8)"
  fi
  poll 10 lines_at_least "$T/stale.out" $((LINES_BEFORE + 3)) \
    && pass "the feed reader gets ticks again after wheel plugin restart" \
    || fail "the feed reader gets ticks again after wheel plugin restart"
  w plugin restart nosuch --space "$S8" > /dev/null 2> "$T/restart-bad.err"
  [ $? -ne 0 ] && grep -q 'nosuch' "$T/restart-bad.err" && pass "wheel plugin restart of an unknown plugin fails with a message" \
    || fail "wheel plugin restart of an unknown plugin fails with a message: $(cat "$T/restart-bad.err")"
else
  fail "kill -9 on the backend: no first tick"
fi

# 18 restart = "never": a killed backend stays exited, plugin ls says so, and the next read starts it.
make_pane "$T/proj9"
S9=$SPACE
P9=$PANE
PANE=$PANE1
if start_feed never "$P9" clock.now; then
  PID9=$(backend_pid "$S9")
  kill -9 "$PID9"
  poll 5 ls_has "$S9" "not restarted (restart = never)" \
    && pass "plugin ls says a restart = never backend is not restarted" \
    || fail "plugin ls says a restart = never backend is not restarted: $(w plugin ls --space "$S9" 2>&1)"
  sleep 2.5
  w plugin ls --space "$S9" 2>&1 | grep -q 'backend: exited' && stays_down "$S9" \
    && pass "the restart = never backend is still down after the backoff would have passed" \
    || fail "the restart = never backend is still down after the backoff would have passed"
  grep -q '"stale":true' "$T/never.out" && pass "its reader was sent the last value stale" \
    || fail "its reader was sent the last value stale"
  w plugin feed "$P9" clock.now --count 2 > "$T/never2.out" 2>&1
  [ -n "$(backend_pid "$S9")" ] && [ "$(backend_pid "$S9")" != "$PID9" ] \
    && pass "the next feed read starts a new process for a restart = never backend" \
    || fail "the next feed read starts a new process for a restart = never backend"
else
  fail "restart = never: no first tick"
fi

# 19 on-failure: a backend that exits with status 0 is not started again.
make_pane "$T/proj10" quiet
P10=$PANE
S10=$SPACE
PANE=$PANE1
w plugin feed "$P10" quiet.x --count 1 > /dev/null 2> "$T/quiet.err"
poll 10 ls_has "$S10" 'not restarted (restart = on-failure)'
sleep 2.5
w plugin ls --space "$S10" > "$T/quiet-ls.txt" 2>&1
grep -q 'exited with status 0 .*; not restarted (restart = on-failure)' "$T/quiet-ls.txt" \
  && ! grep -q 'restarting in' "$T/quiet-ls.txt" \
  && pass "a backend that exits with status 0 is not restarted under on-failure, and none is pending after a wait" \
  || fail "a backend that exits with status 0 is not restarted under on-failure: $(cat "$T/quiet-ls.txt")"
[ "$(grep -c 'starting `exit 0`' "$T/logs/plugins/quiet.log")" -eq 1 ] \
  && pass "its process was started once" || fail "its process was started once"

# 20 a read during the backoff of a crash-looping backend is refused with the time left; the wait
# is not skipped. The first read starts the process, which exits at once; a read in the wait after
# it is the refusal (a read after the wait starts a process and fails in its own way).
make_pane "$T/proj11" crash
P11=$PANE
PANE=$PANE1
refused_in_backoff() {
  w plugin feed "$P11" crash.x --count 1 > "$T/crash.out" 2> "$T/crash.err"
  grep -q 'backend of crash is restarting in [0-9][0-9]* s: exited with status 1' "$T/crash.err"
}
poll 30 refused_in_backoff \
  && pass "a feed read during a crash-looping backend's backoff is refused with restarting in N s" \
  || fail "a feed read during a crash-looping backend's backoff is refused: $(cat "$T/crash.err")"
cat "$T/crash.err"

# 21 a change to a watched file starts a backend that exited and is not restarted by its policy.
make_pane "$T/proj12"
S12=$SPACE
P12=$PANE
PANE=$PANE1
if start_feed touch "$P12" clock.now; then
  PID12=$(backend_pid "$S12")
  kill -9 "$PID12"
  poll 5 ls_has "$S12" "not restarted (restart = never)"
  sleep 2.5
  stays_down "$S12" && pass "the restart = never backend stays down with no restart pending" \
    || fail "the restart = never backend stays down with no restart pending: $(w plugin ls --space "$S12" 2>&1)"
  echo "# touched" >> "$T/proj12/.wheel/plugins/clock/backend.py"
  poll 15 backend_pid_not "$S12" "$PID12" \
    && pass "touching the watched file starts an exited restart = never backend" \
    || fail "touching the watched file starts an exited restart = never backend: $(w plugin ls --space "$S12" 2>&1)"
else
  fail "touching the watched file starts an exited backend: no first tick"
fi

# 22 a restart runs the manifest as it is now. The fixture's command prints a marker line to its
# log and its watch is `*`: editing the manifest is a watched change.
make_pane "$T/proj13"
S13=$SPACE
P13=$PANE
PANE=$PANE1
M13="$T/proj13/.wheel/plugins/clock/wheel-plugin.toml"
CLOCKLOG="$T/logs/plugins/clock.log"
if start_feed spec "$P13" clock.now; then
  has_line "$CLOCKLOG" marker-one && pass "the backend ran the manifest's command" || fail "the backend ran the manifest's command"
  sed -i '' 's/marker-one/marker-two/' "$M13"
  poll 20 has_line "$CLOCKLOG" marker-two \
    && pass "a watched edit of the manifest's command restarts with the new command" \
    || fail "a watched edit of the manifest's command restarts with the new command"
  # The same edit narrows the watch to backend.py: the new patterns apply from the next change.
  sed -i '' -e 's/marker-two/marker-three/' -e 's/^watch = .*/watch = ["backend.py"]/' "$M13"
  poll 20 has_line "$CLOCKLOG" marker-three \
    && pass "the restart after an edit that changes the watch runs the new command" \
    || fail "the restart after an edit that changes the watch runs the new command"
  poll 20 backend_up "$S13"
  PID13=$(backend_pid "$S13")
  sed -i '' 's/marker-three/marker-four/' "$M13"
  sleep 1.5
  [ "$(backend_pid "$S13")" = "$PID13" ] && ! has_line "$CLOCKLOG" marker-four \
    && pass "the narrowed watch no longer names the manifest: its edit restarts nothing" \
    || fail "the narrowed watch no longer names the manifest: its edit restarts nothing"
  kill -9 "$PID13"
  poll 30 has_line "$CLOCKLOG" marker-four \
    && pass "a restart after a crash runs the manifest as it is now" \
    || fail "a restart after a crash runs the manifest as it is now"
  # A manifest that no longer parses: the next restart is refused and says why.
  poll 20 backend_up "$S13"
  PID13=$(backend_pid "$S13")
  echo 'broken [[[' > "$M13"
  echo "# touched" >> "$T/proj13/.wheel/plugins/clock/backend.py"
  poll 10 grep -q 'not restarted: wheel-plugin.toml' "$CLOCKLOG" \
    && pass "a manifest that does not parse is not launched, and the plugin log says why" \
    || fail "a manifest that does not parse is not launched, and the plugin log says why: $(tail -3 "$CLOCKLOG")"
  kill -0 "$PID13" 2>/dev/null \
    && pass "the process that runs is left running" || fail "the process that runs is left running"
else
  fail "a restart runs the manifest as it is now: no first tick"
fi

echo
if [ "$FAILS" -eq 0 ]; then echo "all checks passed"; else echo "$FAILS check(s) failed"; fi
exit "$FAILS"
