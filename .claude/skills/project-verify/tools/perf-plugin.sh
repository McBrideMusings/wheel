#!/bin/bash
# What one event costs a plugin pane, in the hidden GPUI app (never shown, never focused) against a throwaway
# wheeld: the wheelhouse Beads pane over a project's real beads.
#   keystroke  five `set`s of the search field ("w" .. "wheel"), then one back to ""
#   press      five presses of a Button that changes the script's state
# Each event prints the Luau `update` and `view` ms, the reconcile ms (the view into the tree SwiftUI draws), the nodes
# written, the renders it ran, and the component bodies and Table cell bodies SwiftUI evaluated in the second after it.
#
#   perf-plugin.sh [project dir, default /Users/pierce/Projects/wheel] [app, default clients/gpui/target/app/Wheel Dev.app]
set -u
ROOT=$(cd "$(dirname "$0")/../../../.." && pwd)
PROJ=${1:-/Users/pierce/Projects/wheel}
APP=${2:-$ROOT/clients/gpui/target/app/Wheel Dev.app}
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=${PERF_PLUGIN_DIR:-/private/tmp/claude/wheel/perf-plugin}
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
PORT=$(free_port)
DEBUG=$(free_port)
PIDS=()
cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null; done
  sleep 0.5
  for pid in "${PIDS[@]}"; do kill -9 "$pid" 2>/dev/null; done
}
trap cleanup EXIT

rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/plugins"
: > "$T/machine.toml"
ln -s "$ROOT/plugins/wheelhouse" "$T/plugins/wheelhouse"

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

SPACE=$(w space new "$PROJ")
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
w pane kind "$PANE" plugin --plugin wheelhouse --plugin-pane beads > /dev/null

env -u WHEEL_SPACE -u WHEEL_PANE WHEEL_HIDDEN=1 WHEEL_DEBUG_PORT=$DEBUG WHEEL_URL="ws://127.0.0.1:$PORT/ws" \
  WHEEL_TOKEN_FILE="$T/support/token" WHEEL_CLIENT_SPACE="$SPACE" \
  WHEEL_GPUI_BRIDGE="$APP/Contents/MacOS/libWheelGpuiBridge.dylib" \
  "$APP/Contents/MacOS/wheel-gpui" --space "$SPACE" > "$T/app.out" 2>&1 &
PIDS+=($!)

# Waits until the pane draws table rows, then names the search field and the button the events use.
for _ in $(seq 1 150); do
  dbg --data-urlencode action=state > "$T/state.json" 2>/dev/null
  python3 - "$T/state.json" > "$T/ids" 2>/dev/null <<'PY' && break
import json, sys
d = json.load(open(sys.argv[1]))
cs = d.get("components", [])
rows = [c for c in cs if c.get("type") == "TableRow"]
field = next((c for c in cs if c.get("type") == "TextField"), None)
if not rows or not field: sys.exit(1)
print(field["id"], len(rows), len(cs))
PY
  sleep 0.4
done
[ -s "$T/ids" ] || { echo "FAIL the beads pane drew no rows: $(head -c 400 "$T/state.json")"; exit 1; }
read -r FIELD ROWS COMPONENTS < "$T/ids"
echo "beads pane: $ROWS rows, $COMPONENTS components, search field $FIELD"
sleep 2

perf() { dbg --data-urlencode action=state | python3 -c 'import json,sys; p=json.load(sys.stdin)["perf"]; print(p["renders"], p["bodies"], p["update_ms"], p["view_ms"], p["reconcile_ms"], p["nodes"], p["cells"])'; }
# event <label> <dbg args...>: one event, then a second for SwiftUI to draw it.
event() {
  local label=$1; shift
  read -r r0 b0 _ _ _ _ c0 < <(perf)
  dbg "$@" > /dev/null
  sleep 1
  read -r r1 b1 u v c n c1 < <(perf)
  printf '%-10s %-8s update %7.2f ms  view %7.2f ms  reconcile %7.2f ms  nodes %5d  renders %d  bodies %5d  cells %5d\n' "$label" "$EVENT" "$u" "$v" "$c" "$n" $((r1 - r0)) $((b1 - b0)) $((c1 - c0))
  # The limits project-perf's "plugin keystroke" scenario sets; the whole-pane rewrite this replaced wrote 96 nodes and ran 24-27 bodies.
  if [ "$n" -gt "$MAX_NODES" ] || [ $((b1 - b0)) -gt "$MAX_BODIES" ]; then echo "FAIL $label $EVENT: $n nodes (limit $MAX_NODES), $((b1 - b0)) bodies (limit $MAX_BODIES)"; FAILS=$((FAILS + 1)); fi
}
MAX_NODES=${PERF_MAX_NODES:-20}
MAX_BODIES=${PERF_MAX_BODIES:-12}
FAILS=0

EVENT=keystroke
for text in w wh whe whee wheel ""; do
  event "\"$text\"" --data-urlencode action=set --data-urlencode "component=$FIELD" --data-urlencode "value=\"$text\""
done
EVENT=press
BUTTON=${PERF_BUTTON:-}
if [ -z "$BUTTON" ]; then
  dbg --data-urlencode action=state > "$T/state.json"
  # A status box (a Button whose first Text is its count): it toggles a list filter, the script's own state, and runs
  # nothing outside the pane. The project's buttons, which run commands, have words for labels.
  BUTTON=$(python3 - "$T/state.json" <<'PY'
import json, sys
cs = json.load(open(sys.argv[1]))["components"]
for i, c in enumerate(cs):
    if c.get("type") != "Button": continue
    t = next((x.get("text", "") for x in cs[i + 1:i + 5] if x.get("type") == "Text"), "")
    if t.isdigit(): print(c["id"]); break
PY
)
  [ -n "$BUTTON" ] || { echo "FAIL no status box to press"; exit 1; }
fi
echo "button: $BUTTON"
for i in 1 2 3 4 5; do event "#$i" --data-urlencode action=press --data-urlencode "component=$BUTTON"; done
[ "$FAILS" -eq 0 ] && echo "ok   every event within $MAX_NODES nodes and $MAX_BODIES bodies"
exit $((FAILS > 0))
