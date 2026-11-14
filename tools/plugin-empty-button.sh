#!/bin/bash
# Check wheel-pmf2.2.6: an empty pane's plugin button, pressed through the hidden GPUI app's debug server.
# A throwaway wheeld with two projects: A holds a project plugin (.wheel/plugins/onlya), B holds none. One hidden app
# is attached to each space (never shown, never focused). The check
#   1. presses sample/counter on A's empty pane (`POST /empty/plugin`) and reads the pane's kind from wheeld
#      (`wheel pane ls`) and its drawn script from the app (`POST /plugin?action=state`): the counter heading;
#   2. confirms A's empty pane lists onlya/main in /state `plugins` and B's does not, and that pressing it on B is refused.
#
#   tools/plugin-empty-button.sh [path to a Wheel*.app holding wheel-gpui and libWheelGpuiBridge.dylib]
#   default: clients/gpui/target/app/Wheel Dev.app (build it with `python3 clients/gpui/tools/build.py dev`)
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP=${1:-$ROOT/clients/gpui/target/app/Wheel Dev.app}
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=${PT_DIR:-/private/tmp/claude/wheel/plugin-empty-button}
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
PORT=$(free_port)
DEBUG_A=$(free_port)
DEBUG_B=$(free_port)
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
mkdir -p "$T/support" "$T/logs" "$T/a/.wheel/plugins/onlya/panes" "$T/b"
: > "$T/machine.toml"
printf '[plugin]\nid = "onlya"\nversion = "0.1.0"\n\n[[pane]]\nid = "main"\ntitle = "Only A"\nscript = "panes/main.luau"\n' > "$T/a/.wheel/plugins/onlya/wheel-plugin.toml"
printf 'return { init = function() return {} end, update = function() end, view = function() return ui.Text("only a") end }\n' > "$T/a/.wheel/plugins/onlya/panes/main.luau"

env -u WHEEL_SPACE -u WHEEL_PANE "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
  --log-dir "$T/logs" --cli "$WHEEL" > "$T/wheeld.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { env -u WHEEL_SPACE -u WHEEL_PANE "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }
AUTH="Authorization: Bearer $(cat "$T/support/token")"

SPACE_A=$(w space new "$T/a")
SPACE_B=$(w space new "$T/b")
PANE_A=$(w pane ls | awk -v s="$SPACE_A" '$1 == s {print $2; exit}')
PANE_B=$(w pane ls | awk -v s="$SPACE_B" '$1 == s {print $2; exit}')

launch() { # space debug-port
  env -u WHEEL_SPACE -u WHEEL_PANE WHEEL_HIDDEN=1 WHEEL_DEBUG_PORT=$2 WHEEL_URL="ws://127.0.0.1:$PORT/ws" \
    WHEEL_TOKEN_FILE="$T/support/token" WHEEL_CLIENT_SPACE="$1" \
    WHEEL_GPUI_BRIDGE="$APP/Contents/MacOS/libWheelGpuiBridge.dylib" \
    "$APP/Contents/MacOS/wheel-gpui" --space "$1" > "$T/app-$2.out" 2>&1 &
  PIDS+=($!)
}
launch "$SPACE_A" "$DEBUG_A"
launch "$SPACE_B" "$DEBUG_B"
dbg() { curl -s -H "$AUTH" "http://127.0.0.1:$1$2"; }
post() { curl -s -w ' [%{http_code}]' -X POST -H "$AUTH" "http://127.0.0.1:$1$2"; }
offers() { # debug-port space: the plugin pane buttons the app lists for the space, once plugin_ls has answered
  for _ in $(seq 1 100); do
    out=$(dbg "$1" /state | python3 -c 'import json,sys
try: print(" ".join(json.load(sys.stdin)["plugins"].get(sys.argv[1], [])))
except Exception: pass' "$2")
    [ -n "$out" ] && break
    sleep 0.2
  done
  echo "$out"
}

OFFER_A=$(offers "$DEBUG_A" "$SPACE_A")
OFFER_B=$(offers "$DEBUG_B" "$SPACE_B")
echo "buttons on A's empty pane: $OFFER_A"
echo "buttons on B's empty pane: $OFFER_B"
case " $OFFER_A " in *" sample/counter "*) case " $OFFER_A " in *" onlya/main "*) pass "2a A's empty pane offers sample/counter and the project plugin onlya/main";; *) fail "2a A offers: $OFFER_A";; esac;; *) fail "2a A offers: $OFFER_A";; esac
case " $OFFER_B " in *" sample/counter "*) pass "2b B's empty pane offers sample/counter";; *) fail "2b B offers: $OFFER_B";; esac
case " $OFFER_B " in *onlya*) fail "2c B's empty pane offers the project plugin of A: $OFFER_B";; *) pass "2c B's empty pane offers no onlya button";; esac
R=$(post "$DEBUG_B" "/empty/plugin?pane=$PANE_B&plugin=onlya&plugin_pane=main")
echo "press onlya/main on B: $R"
case "$R" in *"[404]"*) pass "2d pressing onlya/main on B is refused";; *) fail "2d press on B answered $R";; esac
w pane ls | grep -q "$PANE_B.*empty" && pass "2e B's pane is still empty" || fail "2e B's pane changed: $(w pane ls | grep "$PANE_B")"

echo "before: $(w pane ls | grep "$PANE_A")"
R=$(post "$DEBUG_A" "/empty/plugin?pane=$PANE_A&plugin=sample&plugin_pane=counter")
echo "press sample/counter on A: ${R##* }"
case "$R" in *"[200]"*) pass "1a the press is accepted";; *) fail "1a press answered $R";; esac
KIND=""
for _ in $(seq 1 50); do
  KIND=$(w pane ls | grep "$PANE_A" | head -1)
  case "$KIND" in *plugin*) break;; esac
  sleep 0.2
done
echo "after: $KIND"
case "$KIND" in *plugin*) pass "1b wheeld lists the pane as a plugin pane";; *) fail "1b pane is: $KIND";; esac
TEXTS=""
for _ in $(seq 1 50); do
  post "$DEBUG_A" "/plugin?pane=$PANE_A&action=state" | sed 's/ \[[0-9]*\]$//' > "$T/plugin.json"
  TEXTS=$(python3 - "$T/plugin.json" <<'PY'
import json, sys
try:
    s = json.load(open(sys.argv[1]))
    print(json.dumps([c["text"] for c in s.get("components", []) if isinstance(c.get("text"), str)]))
except Exception:
    pass
PY
)
  case "$TEXTS" in *"Count 0"*) break;; esac
  sleep 0.2
done
echo "drawn texts: $TEXTS"
case "$TEXTS" in *"Count 0"*"Add one"*) pass "1c the pane draws the sample counter script";; *) fail "1c drawn texts: $TEXTS";; esac
R=$(post "$DEBUG_A" "/empty/plugin?pane=$PANE_A&plugin=sample&plugin_pane=counter")
case "$R" in *"[404]"*) pass "1d the button is gone once the pane is not empty";; *) fail "1d second press answered $R";; esac
PNG="$T/counter.png"
w pane shot "$PANE_A" "$PNG" > "$T/shot.out" 2>&1
BYTES=$(stat -f %z "$PNG" 2>/dev/null || echo 0)
echo "pane shot: $PNG ($BYTES bytes)"
[ "$BYTES" -gt 5000 ] && pass "1e the pane drew: $PNG" || fail "1e no picture of the pane: $(cat "$T/shot.out")"
exit $FAILS
