#!/bin/bash
# Check wheel-pmf2.2.5: a plugin pane survives wheeld dropping the bridge's socket.
# A throwaway wheeld with the clock fixture plugin and the hidden GPUI app (never shown, never focused) attached.
# Case 1: wheeld is killed; the pane's error reads "lost the connection to wheeld ... retrying" and the last view stays.
# Case 2: wheeld starts again on the same port and support dir; the script reloads, the error clears, the pane's state is kept.
#
#   tools/plugin-reconnect.sh [path to a Wheel*.app holding wheel-gpui and libWheelGpuiBridge.dylib]
#   default: clients/gpui/target/app/Wheel Dev.app, else /Applications/Wheel Dev.app
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP=${1:-$ROOT/clients/gpui/target/app/Wheel Dev.app}
[ -d "$APP" ] || APP="/Applications/Wheel Dev.app"
. "$ROOT/tools/fixtures/plugins/clock-copy.sh"
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=${PR_DIR:-/private/tmp/claude/wheel/prc}
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
PORT=$(free_port)
DEBUG=$(free_port)
PIDS=()
WPID=
FAILS=0
cleanup() {
  for pid in "${PIDS[@]}" $WPID; do kill "$pid" 2>/dev/null; done
  sleep 0.5
  for pid in "${PIDS[@]}" $WPID; do kill -9 "$pid" 2>/dev/null; done
  pkill -f "$T/.*backend.py" 2>/dev/null
}
trap cleanup EXIT
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }

rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/proj/.wheel/plugins"
: > "$T/machine.toml"
clock_copy "$T/proj/.wheel/plugins/clock"

start_wheeld() {
  env -u WHEEL_SPACE -u WHEEL_PANE "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
    --log-dir "$T/logs" --cli "$WHEEL" >> "$T/wheeld.out" 2>&1 &
  WPID=$!
  for _ in $(seq 1 100); do
    [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
    sleep 0.2
  done
}
start_wheeld
w() { env -u WHEEL_SPACE -u WHEEL_PANE "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }
AUTH="Authorization: Bearer $(cat "$T/support/token")"
appstate() { curl -s -H "$AUTH" "http://127.0.0.1:$DEBUG/state" > "$T/state.json" 2>/dev/null; }
dbg() { curl -s -G -H "$AUTH" -X POST "http://127.0.0.1:$DEBUG/plugin" --data-urlencode "pane=$PANE" "$@"; }

SPACE=$(w space new "$T/proj")
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
w pane kind "$PANE" plugin --plugin clock --plugin-pane main > /dev/null

env -u WHEEL_SPACE -u WHEEL_PANE WHEEL_HIDDEN=1 WHEEL_DEBUG_PORT=$DEBUG WHEEL_URL="ws://127.0.0.1:$PORT/ws" \
  WHEEL_TOKEN_FILE="$T/support/token" WHEEL_CLIENT_SPACE="$SPACE" \
  WHEEL_GPUI_BRIDGE="$APP/Contents/MacOS/libWheelGpuiBridge.dylib" \
  "$APP/Contents/MacOS/wheel-gpui" --space "$SPACE" > "$T/app.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do appstate; [ -s "$T/state.json" ] && break; sleep 0.2; done

# Prints "error|reloads|out-text|tick-text" of the plugin pane, read from the app's /plugin query.
snap() {
  dbg --data-urlencode action=state > "$T/pane.json" 2>/dev/null
  python3 - "$T/pane.json" <<'PY'
import json, sys
try: d = json.load(open(sys.argv[1]))
except Exception: print("none|0||"); sys.exit()
texts = [c.get("text", "") if isinstance(c, dict) else str(c) for c in d.get("components", [])]
out = [t for t in texts if t.startswith("result:") or t.startswith("error:")]
tick = [t for t in texts if t.startswith("Tick")]
print("%s|%s|%s|%s" % (d.get("error"), d.get("reloads"), out[0] if out else "", tick[0] if tick else ""))
PY
}
# wait_for <seconds> <python condition over e (error), r (reloads), o (out), t (tick)>
wait_for() {
  local end=$((SECONDS + $1))
  while [ $SECONDS -lt $end ]; do
    IFS='|' read -r e r o t <<< "$(snap)"
    python3 -c "import sys; e,r,o,t=sys.argv[1:5]; sys.exit(0 if ($2) else 1)" "$e" "$r" "$o" "$t" && return 0
    sleep 0.5
  done
  return 1
}

wait_for 30 'e == "None" and t.startswith("Tick")' && pass "0a the pane loaded its script: $(snap)" || fail "0a the pane never loaded: $(snap)"
dbg --data-urlencode action=press --data-urlencode component=root.4 > "$T/press.json"
wait_for 20 'o == "result: 1"' && pass "0b Echo pressed: $(snap)" || fail "0b Echo did not set the state: $(snap) $(cat "$T/press.json")"
# The window snapshot pastes the bridge's own drawing of each native pane over GPUI's frame; the reply says which it drew.
shot() {
  curl -s -H "$AUTH" -X POST "http://127.0.0.1:$DEBUG/window/snapshot?path=$T/$1.png" > "$T/$1.json"
  python3 -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1])).get("native_panes", {}).get(sys.argv[2]) == "drawn" else 1)' "$T/$1.json" "$PANE" \
    && pass "$2 the $1 snapshot drew the plugin pane's body: $T/$1.png" || fail "$2 the $1 snapshot did not draw the pane: $(cat "$T/$1.json")"
}
shot before 0c
IFS='|' read -r E0 R0 O0 T0 <<< "$(snap)"

# Case 1: wheeld stops.
kill -9 "$WPID"
wait "$WPID" 2>/dev/null
WPID=
wait_for 20 '"lost the connection to wheeld" in e'
IFS='|' read -r E1 R1 O1 T1 <<< "$(snap)"
echo "after kill: error=$E1 reloads=$R1 out=$O1 view=$T1"
case "$E1" in *"lost the connection to wheeld"*"retrying"*) pass "1a the pane's error reads: $E1" ;; *) fail "1a error after the kill: $E1" ;; esac
[ "$O1" = "result: 1" ] && [ -n "$T1" ] && pass "1b the last view stays: \"$T1\", \"$O1\"" || fail "1b view changed: \"$T1\" \"$O1\" (was \"$T0\" \"$O0\")"
shot lost 1c

# Case 2: wheeld comes back.
start_wheeld
wait_for 90 'e == "None" and int(r) > int("'"$R0"'")'
IFS='|' read -r E2 R2 O2 T2 <<< "$(snap)"
echo "after restart: error=$E2 reloads=$R2 (was $R0) out=$O2 view=$T2"
[ "$R2" -gt "$R0" ] 2>/dev/null && pass "2a the script reloaded: reloads $R0 -> $R2" || fail "2a reloads $R0 -> $R2"
[ "$E2" = None ] && pass "2b the error cleared" || fail "2b error still: $E2"
[ "$O2" = "result: 1" ] && pass "2c the pane's state is kept: \"$O2\"" || fail "2c state after reconnect: \"$O2\""
shot after 2d
echo "snapshots: $T/before.png $T/lost.png $T/after.png"
exit $FAILS
