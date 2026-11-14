#!/bin/bash
# Check wheel-pmf2.12: the GPUI client's Beads plugin pane loads when the `wheelhouse.beads` feed message is over 1 MiB.
# A throwaway wheeld with the wheelhouse plugin, a temp project holding a copy of this repo's beads (bd export | bd import),
# and the hidden GPUI app (never shown, never focused) driven through its debug server: the pane's drawn view must hold bead
# rows within seconds and the pane header must hold no "lost the connection" error.
#
#   tools/gpui-large-feed.sh [path to Wheel Dev.app]   # default clients/gpui/target/app/Wheel Dev.app; run after `clients/gpui/tools/build.py dev`
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP=${1:-$ROOT/clients/gpui/target/app/Wheel Dev.app}
MAIN=${BEADS_SOURCE:-/Users/pierce/Projects/wheel}
T=${LF_DIR:-/private/tmp/claude/Projects-wheel/swarm-wheel-pmf2.12/lf}
WHEELD=$ROOT/target/debug/wheeld
WHEEL=$ROOT/target/debug/wheel
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
PORT=$(free_port)
DEBUG=$(free_port)
PIDS=()
FAILS=0
cleanup() { for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null; done; sleep 0.5; for pid in "${PIDS[@]}"; do kill -9 "$pid" 2>/dev/null; done; }
trap cleanup EXIT
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }

rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/home/.wheel" "$T/plugins" "$T/proj"
for f in .zshenv .zprofile .zshrc .zlogin .bash_profile .bashrc .profile; do
  [ -e "$HOME/$f" ] && ln -s "$HOME/$f" "$T/home/$f"
done
ln -s "$ROOT/plugins/wheelhouse" "$T/plugins/wheelhouse"
: > "$T/machine.toml"

git -C "$T/proj" init -q
( cd "$T/proj" && bd init --quiet --prefix wheel > /dev/null 2>&1 )
( cd "$MAIN" && bd export > "$T/beads.jsonl" 2>/dev/null )
( cd "$T/proj" && bd import "$T/beads.jsonl" > "$T/import.out" 2>&1 )
echo "imported $(wc -l < "$T/beads.jsonl") lines from $MAIN: $(tail -1 "$T/import.out")"

HOME="$T/home" "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
  --log-dir "$T/logs" --cli "$WHEEL" > "$T/wheeld.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }

SPACE=$(w space new "$T/proj")
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
w pane kind "$PANE" plugin --plugin wheelhouse --plugin-pane beads > /dev/null
WHEEL_HIDDEN=1 WHEEL_DEBUG_PORT=$DEBUG WHEEL_URL="ws://127.0.0.1:$PORT/ws" WHEEL_TOKEN_FILE="$T/support/token" \
  WHEEL_CLIENT_SPACE="$SPACE" WHEEL_GPUI_BRIDGE="$APP/Contents/MacOS/libWheelGpuiBridge.dylib" \
  "$APP/Contents/MacOS/wheel-gpui" --space "$SPACE" > "$T/app.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do curl -s -H "Authorization: Bearer $(cat "$T/support/token")" "http://127.0.0.1:$DEBUG/state" > /dev/null 2>&1 && break; sleep 0.2; done

START=$(date +%s)
LOADED=no
for _ in $(seq 1 100); do
  curl -s -H "Authorization: Bearer $(cat "$T/support/token")" -X POST "http://127.0.0.1:$DEBUG/plugin?pane=$PANE&action=state" > "$T/pane.json" 2>/dev/null
  if python3 - "$T/pane.json" <<'PY'
import json, sys
try:
    text = open(sys.argv[1]).read()
    d = json.loads(text)
except Exception:
    sys.exit(1)
import re
texts = [c.get('text', '') for c in d.get('components', [])]
print('pane text:', [t for t in texts if re.search(r'listed', t)], 'error:', d.get('error'))
sys.exit(0 if any(re.fullmatch(r'\d+ listed.*', t) for t in texts) and d.get('error') is None else 1)
PY
  then LOADED=yes; break; fi
  sleep 0.5
done
echo "pane loaded: $LOADED after $(( $(date +%s) - START ))s"
curl -s -H "Authorization: Bearer $(cat "$T/support/token")" "http://127.0.0.1:$DEBUG/state" > "$T/state.json"
BYTES=$(python3 -c 'import json, sys; d = json.load(open(sys.argv[1])); print(max([len(v) for k, v in d.get("feeds", {}).items() if k.startswith("wheelhouse.beads")] or [0]))' "$T/pane.json")
echo "wheelhouse.beads feed value held by the client: $BYTES bytes (the limit that dropped the socket: 1048576)"
[ "$BYTES" -gt 1048576 ] && pass "the feed value the pane holds is over 1 MiB" || fail "the feed value is $BYTES bytes, not over 1 MiB"
python3 - "$T/state.json" "$PANE" <<'PY'
import json, sys
s = json.load(open(sys.argv[1]))
print({k: v for k, v in s.items() if 'plugin' in k.lower() or 'error' in k.lower()})
PY
[ "$LOADED" = yes ] && pass "Beads pane draws bead rows from the 1 MiB+ feed" || fail "Beads pane still on Reading the beads"
curl -s -H "Authorization: Bearer $(cat "$T/support/token")" -X POST "http://127.0.0.1:$DEBUG/window/snapshot?path=$T/pane.png" > /dev/null
echo "snapshot: $T/pane.png"
AUTH="Authorization: Bearer $(cat "$T/support/token")"
PICKER=$(python3 -c 'import json, sys; d = json.load(open(sys.argv[1])); print(next(c["id"] for c in d["components"] if c["type"] == "ChoicePicker"))' "$T/pane.json")
curl -s -G -H "$AUTH" -X POST "http://127.0.0.1:$DEBUG/plugin" --data-urlencode "pane=$PANE" --data-urlencode "action=set" --data-urlencode "path=/$PICKER" --data-urlencode 'json=["graph"]' > "$T/set.json"
GRAPH=no
for _ in $(seq 1 60); do
  curl -s -H "$AUTH" -X POST "http://127.0.0.1:$DEBUG/plugin?pane=$PANE&action=state" > "$T/graph.json"
  if python3 - "$T/graph.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
canvas = [c for c in d['components'] if c['type'] == 'Canvas']
print('Canvas components:', len(canvas), 'error:', d.get('error'), canvas[:1])
sys.exit(0 if canvas and d.get('error') is None else 1)
PY
  then GRAPH=yes; break; fi
  sleep 0.5
done
curl -s -H "$AUTH" -X POST "http://127.0.0.1:$DEBUG/window/snapshot?path=$T/graph.png" > /dev/null
echo "graph snapshot: $T/graph.png"
[ "$GRAPH" = yes ] && pass "Graph view draws a Canvas on the 1 MiB+ feed" || fail "Graph view drew no Canvas"
exit $FAILS
