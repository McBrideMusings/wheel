#!/bin/bash
# Check wheel-pmf2.3.6.1: the TypeScript SDK's ticker example (sdk/ts/examples/ticker) draws in the GPUI client.
# A throwaway wheeld with the ticker package installed in a project's .wheel/plugins, a plugin pane on it, and the
# hidden GPUI app (never shown, never focused) attached. The check reads the pane's plugin state from the app's debug
# server (`POST /plugin?pane=&action=state`, the plugin op check.rs plugin_panes uses), waits for the heading to count
# up ("Tick <n> at <time>", n rising), and draws the pane with `wheel pane shot`.
#
#   tools/plugin-sdk-pane.sh [path to a Wheel*.app holding wheel-gpui and libWheelGpuiBridge.dylib]
#   default: clients/gpui/target/app/Wheel Dev.app (build it with `python3 clients/gpui/tools/build.py dev`)
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP=${1:-$ROOT/clients/gpui/target/app/Wheel Dev.app}
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=${PT_DIR:-/private/tmp/claude/wheel/sdk-ts-pane}
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
( cd "$ROOT/sdk/ts" && npm install --no-audit --no-fund > "$T/npm.log" 2>&1 && npm run build >> "$T/npm.log" 2>&1 ) \
  && pass "the SDK installs and compiles with tsc" || { fail "the SDK installs and compiles: $(tail -5 "$T/npm.log")"; exit 1; }
: > "$T/machine.toml"
ln -s "$ROOT/sdk/ts/examples/ticker" "$T/proj/.wheel/plugins/ticker"

env -u WHEEL_SPACE -u WHEEL_PANE "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
  --log-dir "$T/logs" --cli "$WHEEL" > "$T/wheeld.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { env -u WHEEL_SPACE -u WHEEL_PANE "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }
AUTH="Authorization: Bearer $(cat "$T/support/token")"

SPACE=$(w space new "$T/proj")
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
w pane kind "$PANE" plugin --plugin ticker --plugin-pane main > /dev/null

env -u WHEEL_SPACE -u WHEEL_PANE WHEEL_HIDDEN=1 WHEEL_DEBUG_PORT=$DEBUG WHEEL_URL="ws://127.0.0.1:$PORT/ws" \
  WHEEL_TOKEN_FILE="$T/support/token" WHEEL_CLIENT_SPACE="$SPACE" \
  WHEEL_GPUI_BRIDGE="$APP/Contents/MacOS/libWheelGpuiBridge.dylib" \
  "$APP/Contents/MacOS/wheel-gpui" --space "$SPACE" > "$T/app.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  curl -s -H "$AUTH" "http://127.0.0.1:$DEBUG/state" > "$T/state.json" 2>/dev/null; [ -s "$T/state.json" ] && break; sleep 0.2
done

# Each poll appends the drawn components' texts to $T/polls.jsonl; ticks are the n of "Tick <n> at <time>".
: > "$T/polls.jsonl"
for _ in $(seq 1 60); do
  curl -s -X POST -H "$AUTH" "http://127.0.0.1:$DEBUG/plugin?pane=$PANE&action=state" > "$T/plugin.json" 2>/dev/null
  python3 - "$T/plugin.json" >> "$T/polls.jsonl" <<'PY'
import json, sys
try:
    s = json.load(open(sys.argv[1]))
except Exception:
    s = {}
print(json.dumps({"ok": s.get("ok"), "error": s.get("error"), "texts": [c["text"] for c in s.get("components", []) if isinstance(c.get("text"), str)],
                  "types": [c.get("type") for c in s.get("components", [])]}))
PY
  python3 - "$T/polls.jsonl" <<'PY' && break
import json, re, sys
ns = []
for line in open(sys.argv[1]):
    p = json.loads(line)
    for t in p["texts"]:
        m = re.match(r"Tick (\d+) at \d\d:\d\d:\d\d$", t)
        if m:
            ns.append(int(m.group(1)))
sys.exit(0 if len(set(ns)) >= 3 and ns[-1] > ns[0] else 1)
PY
  sleep 0.5
done
python3 - "$T/polls.jsonl" <<'PY' > "$T/verdict.txt"
import json, re, sys
ns, last, types = [], None, []
for line in open(sys.argv[1]):
    p = json.loads(line)
    if p["texts"]:
        last, types = p, p["types"]
    for t in p["texts"]:
        m = re.match(r"Tick (\d+) at \d\d:\d\d:\d\d$", t)
        if m and (not ns or ns[-1] != int(m.group(1))):
            ns.append(int(m.group(1)))
rising = len(ns) >= 3 and all(b > a for a, b in zip(ns, ns[1:]))
print("ok" if rising and last and last["error"] is None and "Unsupported" not in types else "ns=%r last=%r" % (ns, last))
print("heading values seen: %r; texts at the end: %r; component types: %r" % (ns, last and last["texts"], types))
PY
cat "$T/verdict.txt"
[ "$(head -1 "$T/verdict.txt")" = ok ] && pass "1a the ticker pane's state in the app shows Tick n counting up, no error, no Unsupported component" \
  || fail "1a the ticker pane never showed a counting heading in the app's plugin state"

PNG="$T/ticker.png"
w pane shot "$PANE" "$PNG" > "$T/shot.out" 2>&1
BYTES=$(stat -f %z "$PNG" 2>/dev/null || echo 0)
echo "pane shot: $PNG ($BYTES bytes)"
[ "$BYTES" -gt 5000 ] && pass "1b the pane drew: $PNG" || fail "1b no picture of the pane: $(cat "$T/shot.out")"
exit $FAILS
