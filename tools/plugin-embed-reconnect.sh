#!/bin/bash
# Check wheel-pmf2.2.8: a plugin pane's embedded terminal and agent chat attach again after wheeld restarts.
# A throwaway wheeld running the fake agent, a plugin pane on the embed fixture (a terminal pane of the space
# and the first agent pane's chat inside the page), and the hidden GPUI app (never shown, never focused) attached.
# Case 1: after the restart the embedded terminal attaches to the term the new layout names, and a
#         `wheel term send` echo reaches it (the /plugin query's embeds lists the term and its bytes grow).
# Case 2: after the restart the embedded agent chat shows the agent again with no error.
#
#   tools/plugin-embed-reconnect.sh [path to a Wheel*.app holding wheel-gpui and libWheelGpuiBridge.dylib]
#   default: clients/gpui/target/app/Wheel Dev.app, else /Applications/Wheel Dev.app
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP=${1:-$ROOT/clients/gpui/target/app/Wheel Dev.app}
[ -d "$APP" ] || APP="/Applications/Wheel Dev.app"
FAKE=${FAKE:-$ROOT/target/debug/wheel-fake-agent}
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=${PR_DIR:-/private/tmp/claude/wheel/pre}
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
printf '[agent.claude]\ncommand = "%s"\n' "$FAKE" > "$T/machine.toml"
mkdir -p "$T/proj/.wheel/plugins/embed"
cp "$ROOT/tools/fixtures/plugins/embed/wheel-plugin.toml" "$ROOT/tools/fixtures/plugins/embed/main.luau" "$T/proj/.wheel/plugins/embed/"

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
TPANE=$(w pane split "$PANE" right)
w pane start "$TPANE" terminal > /dev/null 2>&1
APANE=$(w pane split "$TPANE" down)
w pane start "$APANE" agent --agent claude > "$T/start.txt" 2>&1
w agent send "$APANE" "hello from the mac" > /dev/null 2>&1
w pane kind "$PANE" plugin --plugin embed --plugin-pane main > /dev/null
echo "space $SPACE: plugin $PANE, terminal $TPANE, agent $APANE"

env -u WHEEL_SPACE -u WHEEL_PANE WHEEL_HIDDEN=1 WHEEL_DEBUG_PORT=$DEBUG WHEEL_URL="ws://127.0.0.1:$PORT/ws" \
  WHEEL_TOKEN_FILE="$T/support/token" WHEEL_CLIENT_SPACE="$SPACE" \
  WHEEL_GPUI_BRIDGE="$APP/Contents/MacOS/libWheelGpuiBridge.dylib" \
  "$APP/Contents/MacOS/wheel-gpui" --space "$SPACE" > "$T/app.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do appstate; [ -s "$T/state.json" ] && break; sleep 0.2; done

# Prints "error|term|bytes|agent|agent-error" of the plugin pane's embeds, read from the app's /plugin query.
snap() {
  dbg --data-urlencode action=state > "$T/pane.json" 2>/dev/null
  python3 - "$T/pane.json" <<'PY'
import json, sys
try: d = json.load(open(sys.argv[1]))
except Exception: print("none|None|0|None|None"); sys.exit()
term = next((e for e in d.get("embeds", []) if "term" in e), {})
agent = next((e for e in d.get("embeds", []) if "agent" in e), {})
print("%s|%s|%s|%s|%s" % (d.get("error"), term.get("term"), term.get("bytes", 0), agent.get("agent"), agent.get("error")))
PY
}
# wait_for <seconds> <python condition over e (plugin error), t (term), b (bytes), a (agent), ae (agent error)>
wait_for() {
  local end=$((SECONDS + $1))
  while [ $SECONDS -lt $end ]; do
    IFS='|' read -r e t b a ae <<< "$(snap)"
    python3 -c "import sys; e,t,b,a,ae=sys.argv[1:6]; b=int(b); sys.exit(0 if ($2) else 1)" "$e" "$t" "$b" "$a" "$ae" && return 0
    sleep 0.5
  done
  return 1
}

wait_for 40 'e == "None" and t != "None" and a != "None"' && pass "0a before: terminal and agent attached: $(snap)" || fail "0a before: embeds not attached: $(snap)"
IFS='|' read -r E0 T0 B0 A0 AE0 <<< "$(snap)"
curl -s -H "$AUTH" -X POST "http://127.0.0.1:$DEBUG/window/snapshot?path=$T/before.png" > /dev/null

kill -9 "$WPID"
wait "$WPID" 2>/dev/null
WPID=
wait_for 20 '"lost the connection to wheeld" in e'
start_wheeld
wait_for 90 'e == "None" and t != "None" and a != "None"'
IFS='|' read -r E2 T2 B2 A2 AE2 <<< "$(snap)"
echo "after restart: error=$E2 term=$T2 bytes=$B2 agent=$A2 agent_error=$AE2 (before: term=$T0 bytes=$B0 agent=$A0)"
NEWTERM=$(w status --json | python3 -c '
import json, sys
def walk(v):
    if isinstance(v, dict):
        kind = v.get("kind")
        if v.get("id") == sys.argv[1] and isinstance(kind, dict) and "term" in kind: print(kind["term"]); sys.exit()
        if v.get("id") == sys.argv[1] and "term" in v: print(v["term"]); sys.exit()
        for x in v.values(): walk(x)
    elif isinstance(v, list):
        for x in v: walk(x)
walk(json.load(sys.stdin))' "$TPANE")
echo "the new wheeld's term for $TPANE: $NEWTERM"

# Case 1: the embedded terminal is on the new wheeld's term and a send reaches it.
[ -n "$NEWTERM" ] && [ "$T2" = "$NEWTERM" ] && pass "1a the embed attaches to the new term $NEWTERM" || fail "1a embed term \"$T2\", wheeld's term \"$NEWTERM\""
IFS='|' read -r _ _ BB _ _ <<< "$(snap)"
w term send "$NEWTERM" "echo embed-after-restart" --enter > "$T/send.out" 2>&1
wait_for 20 "b > $BB" && pass "1b the embed's bytes grew after a send: $BB -> $(snap | cut -d'|' -f3)" || fail "1b bytes stayed at $BB: $(snap) $(cat "$T/send.out")"

# Case 2: the embedded agent chat shows the agent again with no error.
[ "$A2" != None ] && [ "$AE2" = None ] && pass "2 the embedded chat shows agent \"$A2\", error $AE2" || fail "2 agent \"$A2\", error \"$AE2\""
curl -s -H "$AUTH" -X POST "http://127.0.0.1:$DEBUG/window/snapshot?path=$T/after.png" > /dev/null
echo "snapshots: $T/before.png $T/after.png"
exit $FAILS
