#!/bin/bash
# Check wheel-pmf2.5.3 in the hidden GPUI app (never shown, never focused): the command backend
# fixture (tools/fixtures/plugins/commands, a manifest and a pane script, no backend) drawn as a
# plugin pane against a throwaway wheeld.
# Case 1: the pane shows the `now` feed's JSON value and it changes on the interval.
# Case 2: Write runs its action, the pane shows the result and the note feed runs again.
# Case 3: Break makes the note feed fail; the pane shows its stderr (with the action's arg) as the
#         error; Mend clears it.
#
#   tools/plugin-commands-pane.sh [path to a Wheel*.app holding wheel-gpui and libWheelGpuiBridge.dylib]
#   default: clients/gpui/target/app/Wheel Dev.app
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP=${1:-$ROOT/clients/gpui/target/app/Wheel Dev.app}
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=${PCP_DIR:-/private/tmp/claude/wheel/pcp}
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
PORT=$(free_port)
DEBUG=$(free_port)
PIDS=()
FAILS=0
cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null; done
  sleep 0.5
  for pid in "${PIDS[@]}"; do kill -9 "$pid" 2>/dev/null; done
}
trap cleanup EXIT
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }

rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/proj/.wheel/plugins"
: > "$T/machine.toml"
cp -R "$ROOT/tools/fixtures/plugins/commands" "$T/proj/.wheel/plugins/cmd"

env -u WHEEL_SPACE -u WHEEL_PANE "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
  --log-dir "$T/logs" --cli "$WHEEL" >> "$T/wheeld.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { env -u WHEEL_SPACE -u WHEEL_PANE "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }
AUTH="Authorization: Bearer $(cat "$T/support/token")"
dbg() { curl -s -G -H "$AUTH" -X POST "http://127.0.0.1:$DEBUG/plugin" --data-urlencode "pane=$PANE" "$@"; }
shot() { w pane shot "$PANE" "$T/$1.png" > "$T/shot-$1.out" 2>&1; }

SPACE=$(w space new "$T/proj")
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
w pane kind "$PANE" plugin --plugin cmd --plugin-pane main > /dev/null

env -u WHEEL_SPACE -u WHEEL_PANE WHEEL_HIDDEN=1 WHEEL_DEBUG_PORT=$DEBUG WHEEL_URL="ws://127.0.0.1:$PORT/ws" \
  WHEEL_TOKEN_FILE="$T/support/token" WHEEL_CLIENT_SPACE="$SPACE" \
  WHEEL_GPUI_BRIDGE="$APP/Contents/MacOS/libWheelGpuiBridge.dylib" \
  "$APP/Contents/MacOS/wheel-gpui" --space "$SPACE" > "$T/app.out" 2>&1 &
PIDS+=($!)

# The pane's texts, one per line, from the app's /plugin state; the pane's error on the first line.
texts() {
  dbg --data-urlencode action=state > "$T/pane.json" 2>/dev/null
  python3 - "$T/pane.json" <<'PY'
import json, sys
try: d = json.load(open(sys.argv[1]))
except Exception: print("error: no state"); sys.exit()
print("pane-error: %s" % d.get("error"))
for c in d.get("components", []):
    if isinstance(c, dict) and c.get("text"):
        print(c["text"])
PY
}
# wait_for <seconds> <grep -E pattern>: succeeds once a text of the pane matches.
wait_for() {
  local end=$((SECONDS + $1))
  while [ $SECONDS -lt $end ]; do
    texts | grep -Eq -- "$2" && return 0
    sleep 0.3
  done
  return 1
}
now_text() { texts | grep -E '^Now [0-9]+$' | head -1; }

# Case 1.
wait_for 40 '^Now [0-9]+$' && N1=$(now_text) && pass "1a the pane shows the JSON feed: $N1" || fail "1a no value: $(texts | tr '\n' '|')"
shot case1
wait_for 5 "^Now [0-9]+$" && sleep 2.2 && N2=$(now_text)
[ -n "${N1:-}" ] && [ -n "${N2:-}" ] && [ "$N1" != "$N2" ] && pass "1b the value changes on the interval: $N1 -> $N2" || fail "1b $N1 -> ${N2:-}"
wait_for 5 '^Note lines: 0, last: none$' && pass "1c the note feed starts empty" || fail "1c $(texts | tr '\n' '|')"

# Case 2: Write (root/Row0/Button0) runs `write` with the draft as ARG_TEXT; the result shows and the note reruns.
dbg --data-urlencode action=press --data-urlencode component=root/Row0/Button0 > "$T/press-write.json"
wait_for 10 '^result: wrote $' && pass "2a the action's result shows: result: wrote" || fail "2a $(texts | tr '\n' '|') $(cat "$T/press-write.json")"
wait_for 10 '^Note lines: 1, last: $' && pass "2b the note feed ran again after the action: Note lines: 1" || fail "2b $(texts | tr '\n' '|')"
shot case2

# Case 3: Break (root/Row0/Button1) sends why = "on purpose"; the note feed fails with its stderr; Mend clears it.
dbg --data-urlencode action=press --data-urlencode component=root/Row0/Button1 > /dev/null
wait_for 10 '^Note error: exit 4: note is broken: on purpose$' && pass "3a the failing run's stderr is the pane's error" || fail "3a $(texts | tr '\n' '|')"
texts | grep -q '^Note lines: 1, last: $' && pass "3b the last good value stays while it fails" || fail "3b $(texts | tr '\n' '|')"
shot case3
dbg --data-urlencode action=press --data-urlencode component=root/Row0/Button2 > /dev/null
wait_for 10 '^Note error: none$' && pass "3c the next good run clears the error" || fail "3c $(texts | tr '\n' '|')"
dbg --data-urlencode action=press --data-urlencode component=root/Row0/Button3 > /dev/null
wait_for 10 '^exit 7: refused: no$' && pass "3d a failing action's exit code and stderr show" || fail "3d $(texts | tr '\n' '|')"
shot case3-cleared

echo "snapshots: $T/case1.png $T/case2.png $T/case3.png $T/case3-cleared.png"
[ $FAILS -eq 0 ] && echo "ALL PASS" || echo "$FAILS FAILED"
exit $FAILS
