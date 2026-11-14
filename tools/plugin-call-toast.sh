#!/bin/bash
# Check wheel-pmf2.3.5.1: a backend's wheel/call `notify` shows a toast in the GPUI client.
# A throwaway wheeld with the clock fixture plugin (needs = [notify]) and the hidden GPUI app (never shown,
# never focused) attached to it. The clock backend's `call` action sends wheel/call notify; the check reads
# the toast (daemon id plugin:clock:<id>, the client keys it notify:<id>) and its text from the app's /state
# and draws the window with it.
#
#   tools/plugin-call-toast.sh [path to a Wheel*.app holding wheel-gpui and libWheelGpuiBridge.dylib]
#   default: clients/gpui/target/app/Wheel Dev.app, else /Applications/Wheel Dev.app
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP=${1:-$ROOT/clients/gpui/target/app/Wheel Dev.app}
[ -d "$APP" ] || APP="/Applications/Wheel Dev.app"
. "$ROOT/tools/fixtures/plugins/clock-copy.sh"
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=${PT_DIR:-/private/tmp/claude/wheel/pct}
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
PORT=$(free_port)
DEBUG=$(free_port)
PIDS=()
FAILS=0
cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null; done
  sleep 0.5
  for pid in "${PIDS[@]}"; do kill -9 "$pid" 2>/dev/null; done
  pkill -f "$T/.*backend.py" 2>/dev/null
}
trap cleanup EXIT
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }

rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/proj/.wheel/plugins"
: > "$T/machine.toml"
clock_copy "$T/proj/.wheel/plugins/clock"
M="$T/proj/.wheel/plugins/clock/wheel-plugin.toml"
sed -i '' -e 's/^actions = \[/actions = ["clock.call", /' -e 's/^watch = .*/&\nneeds = ["notify"]/' "$M"

env -u WHEEL_SPACE -u WHEEL_PANE "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
  --log-dir "$T/logs" --cli "$WHEEL" > "$T/wheeld.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { env -u WHEEL_SPACE -u WHEEL_PANE "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }
AUTH="Authorization: Bearer $(cat "$T/support/token")"
state() { curl -s -H "$AUTH" "http://127.0.0.1:$DEBUG/state" > "$T/state.json" 2>/dev/null; }

SPACE=$(w space new "$T/proj")
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
w pane kind "$PANE" plugin --plugin clock --plugin-pane main > /dev/null

env -u WHEEL_SPACE -u WHEEL_PANE WHEEL_HIDDEN=1 WHEEL_DEBUG_PORT=$DEBUG WHEEL_URL="ws://127.0.0.1:$PORT/ws" \
  WHEEL_TOKEN_FILE="$T/support/token" WHEEL_CLIENT_SPACE="$SPACE" \
  WHEEL_GPUI_BRIDGE="$APP/Contents/MacOS/libWheelGpuiBridge.dylib" \
  "$APP/Contents/MacOS/wheel-gpui" --space "$SPACE" > "$T/app.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do state; [ -s "$T/state.json" ] && break; sleep 0.2; done

toasts() { python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1])).get("toasts", [])))' "$T/state.json" 2>/dev/null; }
state
BEFORE=$(toasts)
echo "toasts before: $BEFORE"

ARGS='{"method":"notify","args":{"id":"pct-1","level":"warning","text":"build finished"}}'
OUT=$(w plugin action "$PANE" clock.call --args "$ARGS" 2> "$T/call.err")
echo "call answer: $OUT"

FOUND=no
for _ in $(seq 1 40); do
  state
  if python3 - "$T/state.json" <<'PY'
import json, sys
t = [x for x in json.load(open(sys.argv[1])).get("toasts", []) if x["id"] == "notify:plugin:clock:pct-1"]
sys.exit(0 if t and t[0]["text"] == "build finished" and t[0]["level"] == "warning" else 1)
PY
  then FOUND=yes; break; fi
  sleep 0.25
done
echo "toasts after: $(toasts)"
[ "$OUT" = '{"result":{}}' ] && pass "1a the call answered {}" || fail "1a call answered: $OUT"
[ "$FOUND" = yes ] && pass "1b the app's /state holds toast notify:plugin:clock:pct-1, warning, \"build finished\"" \
  || fail "1b the toast never showed in the app's /state"
curl -s -H "$AUTH" -X POST "http://127.0.0.1:$DEBUG/window/snapshot?path=$T/toast.png" > "$T/snapshot.json"
echo "snapshot: $T/toast.png ($(stat -f %z "$T/toast.png" 2>/dev/null || echo missing) bytes)"
[ -s "$T/toast.png" ] && pass "1c the window drew with the toast: $T/toast.png" || fail "1c no snapshot: $(cat "$T/snapshot.json")"
exit $FAILS
