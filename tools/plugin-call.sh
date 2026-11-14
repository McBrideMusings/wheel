#!/bin/bash
# Check wheel/call (wheel-pmf2.3.5) against a throwaway wheeld: the clock fixture
# (tools/fixtures/plugins/clock) lists `needs = [pane.open, notify, run:echo hi]` and its `call`
# action sends wheel/call to wheeld. Allowed calls act; a method outside the needs is refused.
# Prints PASS or FAIL per check. Uses no GUI, focus, mouse or keyboard; kills what it starts by pid.
#
#   tools/plugin-call.sh            # uses target/debug/wheeld and target/debug/wheel
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
. "$ROOT/tools/fixtures/plugins/clock-copy.sh"
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=/private/tmp/claude/wheel/pc
PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
FAILS=0
PIDS=()
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }
cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null; done
  sleep 0.5
  for pid in "${PIDS[@]}"; do kill -9 "$pid" 2>/dev/null; done
  pkill -f "$T/.*backend.py" 2>/dev/null
  pkill -f "$T/.*/backend\$" 2>/dev/null
}
trap cleanup EXIT

rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/proj/.wheel/plugins"
: > "$T/machine.toml"
clock_copy "$T/proj/.wheel/plugins/clock"
M="$T/proj/.wheel/plugins/clock/wheel-plugin.toml"
sed -i '' -e 's/^actions = \[/actions = ["clock.call", /' -e 's/^watch = .*/&\nneeds = ["pane.open", "notify", "run:echo hi"]/' "$M"

"$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
  --log-dir "$T/logs" --cli "$WHEEL" > "$T/wheeld.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }
SPACE=$(w space new "$T/proj")
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
w pane kind "$PANE" plugin --plugin clock --plugin-pane main > /dev/null

# call <json {method,args}>: the action's answer, a line of {"result":..} or {"error":..}.
call() { w plugin action "$PANE" clock.call --args "$1" 2> "$T/call.err"; }
panes() { w pane ls | wc -l | tr -d ' '; }

BEFORE=$(panes)
OUT=$(call '{"method":"pane.open","args":{"pane":"'"$PANE"'","url":"https://example.com"}}')
AFTER=$(panes)
NEW=$(echo "$OUT" | python3 -c 'import json,sys; print(json.loads(sys.stdin.readline()).get("result",{}).get("pane",""))')
if [ -n "$NEW" ] && [ "$AFTER" -eq $((BEFORE + 1)) ] && w pane ls | grep -q "$NEW"; then
  pass "1 pane.open opened pane $NEW (panes $BEFORE -> $AFTER)"
else
  fail "1 pane.open: out=$OUT err=$(cat "$T/call.err") panes $BEFORE -> $AFTER"
fi

OUT=$(call '{"method":"notify","args":{"id":"pc-1","level":"warning","text":"build finished"}}')
if [ "$OUT" = '{"result":{}}' ] && grep -rq 'plugin toast sent' "$T/wheeld.out" "$T/logs" 2>/dev/null; then
  pass "2 notify answered {} and wheeld logged 'plugin toast sent'"
else
  fail "2 notify: out=$OUT"
fi

OUT=$(call '{"method":"command.run","args":{"pane":"'"$PANE"'","command":"echo hi"}}')
echo "$OUT" | grep -q '"pane"' && pass "2b command.run of an approved command opened a terminal pane" \
  || fail "2b command.run approved: $OUT"

BEFORE=$(panes)
OUT=$(call '{"method":"command.run","args":{"pane":"'"$PANE"'","command":"rm -rf /"}}')
if echo "$OUT" | grep -q 'run:rm -rf /' && [ "$(panes)" -eq "$BEFORE" ]; then
  pass "3 command.run of a command outside the needs is refused and opens nothing"
else
  fail "3 forbidden command.run: $OUT"
fi

mkdir -p "$T/other"
OTHER=$(w space new "$T/other")
OPANE=$(w pane ls | awk -v s="$OTHER" '$1 == s {print $2; exit}')
OUT=$(call '{"method":"command.run","args":{"pane":"'"$OPANE"'","command":"echo hi"}}')
echo "$OUT" | grep -q 'may not act on pane' \
  && pass "3c a pane in a space with none of the plugin's panes is refused" \
  || fail "3c foreign pane: $OUT"

sed -i '' 's/, "notify"//' "$M"
w plugin restart clock --space "$SPACE" > /dev/null 2>&1
OUT=$(call '{"method":"notify","args":{"text":"x"}}')
echo "$OUT" | grep -q 'does not name \\"notify\\"\|does not name "notify"' \
  && pass "3b notify removed from needs is refused after a restart" \
  || fail "3b notify after removal: $OUT"

echo
if [ "$FAILS" -eq 0 ]; then echo "all checks passed"; else echo "$FAILS check(s) failed"; fi
exit "$FAILS"
