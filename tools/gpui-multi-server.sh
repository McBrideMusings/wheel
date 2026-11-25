#!/bin/bash
# Check wheel-2aln.8 (ADR-0057): the GPUI Mac app keeps one connection per wheeld, shared by its
# windows, and pairs with a remote wheeld. Two throwaway wheelds: A, the app's local one, and B with
# its remote listener on 127.0.0.1. The hidden app (never shown, never focused) is driven through
# its debug server: it pairs with B's code (POST /servers/pair), opens a window on B
# (POST /servers/window) and a second window on A. Then:
#   - A's `wheel client ls` lists one client holding both A windows, B's lists one holding B's;
#   - closing one A window drops only its view from A's list and leaves B's alone;
#   - killing B leaves only B's window Down, drawn at link_opacity 0.5, and A's windows connected.
# The server list goes to a scratch file (WHEEL_GPUI_SERVERS_FILE); the credential goes to the
# login Keychain under "Wheel App Server" and B's id, and the cleanup deletes it. Prints PASS or
# FAIL per check; kills what it starts by pid.
#
# SHOTS=<dir> also draws B's window to <dir>/b-up.png before B is killed and b-down.png after.
#
#   tools/gpui-multi-server.sh [path to Wheel Dev.app]   # default clients/gpui/target/app/Wheel Dev.app; run after `clients/gpui/tools/build.py dev`
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP=${1:-$ROOT/clients/gpui/target/app/Wheel Dev.app}
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=${MS_DIR:-/private/tmp/claude/wheel/gpui-multi-server}
FAILS=0
PIDS=()
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }
port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
cleanup() {
  [ -n "${B_ID:-}" ] && security delete-generic-password -s "Wheel App Server" -a "$B_ID" > /dev/null 2>&1
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
  sleep 0.5
  for p in "${PIDS[@]}"; do kill -9 "$p" 2>/dev/null; done
}
trap cleanup EXIT

# start <dir> <local port> [remote port]: a wheeld; with a remote port it listens there too.
start() {
  mkdir -p "$1/support" "$1/logs"
  if [ -n "${3:-}" ]; then printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$3" > "$1/machine.toml"; else : > "$1/machine.toml"; fi
  "$WHEELD" --listen "127.0.0.1:$2" --support-dir "$1/support" --machine-file "$1/machine.toml" \
    --log-dir "$1/logs" --cli "$WHEEL" >> "$1/wheeld.out" 2>&1 &
  PIDS+=($!)
  for _ in $(seq 1 100); do
    [ -f "$1/support/token" ] && nc -z 127.0.0.1 "$2" > /dev/null 2>&1 && { [ -z "${3:-}" ] || nc -z 127.0.0.1 "$3" > /dev/null 2>&1; } && return
    sleep 0.2
  done
  echo "wheeld in $1 never listened"
}
# on <dir> <port> <args...>: a command on that wheeld's local listener.
on() { local d=$1 p=$2; shift 2; env -u WHEEL_PANE -u WHEEL_SPACE "$WHEEL" --url "ws://127.0.0.1:$p/ws" --token-file "$d/support/token" "$@"; }
# debug <method> <path> [body]: one debug-server request to the hidden app.
debug() { curl -s -X "$1" -H "Authorization: Bearer $(cat "$T/a/support/token")" ${3:+--data-binary "$3"} "http://127.0.0.1:$DEBUG$2"; }
field() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
# windows_of <dir> <port>: every client's windows that wheeld lists (the app is "mac" on its local wheeld and its paired name on a remote one), as "<client count> <window ids…>".
windows_of() { on "$1" "$2" client ls --json | python3 -c '
import json, sys
cs = json.load(sys.stdin)
print(len(cs), " ".join(sorted(w["window"] for c in cs for w in c.get("windows", []))))'; }

rm -rf "$T"
mkdir -p "$T"
AL=$(port); BL=$(port); BR=$(port); DEBUG=$(port)
start "$T/a" "$AL"
start "$T/b" "$BL" "$BR"
B_ID=$(on "$T/b" "$BL" status --json | field "d['host']['id']")
SA=$(on "$T/a" "$AL" space new "$T" --name only-on-a)
SB=$(on "$T/b" "$BL" space new "$T" --name only-on-b)

WHEEL_HIDDEN=1 WHEEL_DEBUG_PORT=$DEBUG WHEEL_URL="ws://127.0.0.1:$AL/ws" WHEEL_TOKEN_FILE="$T/a/support/token" \
  WHEEL_GPUI_SERVERS_FILE="$T/servers.json" WHEEL_GPUI_WINDOWS_FILE="$T/windows.json" \
  WHEEL_GPUI_BRIDGE="$APP/Contents/MacOS/libWheelGpuiBridge.dylib" \
  "$APP/Contents/MacOS/wheel-gpui" --space "$SA" > "$T/app.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do debug GET /state > /dev/null 2>&1 && break; sleep 0.2; done
W1=$(debug GET /state | field "d['window']")

PAIRED=$(debug POST "/servers/pair?name=gpui-check" "$(on "$T/b" "$BL" remote pair --address 127.0.0.1)")
[ "$(echo "$PAIRED" | field "d.get('id')")" = "$B_ID" ] && pass "the app pairs with B ($B_ID)" || fail "pairing: $PAIRED"
grep -q "\"$B_ID\"" "$T/servers.json" && ! grep -qi credential "$T/servers.json" && pass "servers.json lists B and holds no credential" || fail "servers.json: $(cat "$T/servers.json" 2>&1)"
security find-generic-password -s "Wheel App Server" -a "$B_ID" > /dev/null 2>&1 && pass "B's credential is in the Keychain" || fail "B's credential is in the Keychain"

W2=$(debug POST "/servers/window?server=$B_ID" | field "d.get('window')")
debug POST "/action?id=new-window" > /dev/null
W3=
for _ in $(seq 1 50); do
  W3=$(debug GET /state | field "' '.join(sorted(w['id'] for w in d['window_info']['windows']['windows'] if w['id'] != '$W1'))")
  [ -n "$W3" ] && break
  sleep 0.2
done
for _ in $(seq 1 50); do
  [ "$(debug GET "/state?window=$W2" | field "d['connection']")" = connected ] && [ "$(debug GET "/state?window=$W2" | field "d['active']")" = "$SB" ] && break
  sleep 0.2
done
STATE2=$(debug GET "/state?window=$W2")
[ "$(echo "$STATE2" | field "d['window_server']")" = "$B_ID" ] && [ "$(echo "$STATE2" | field "d['active']")" = "$SB" ] && pass "window $W2 shows B's space $SB" || fail "window $W2: server $(echo "$STATE2" | field "d['window_server']"), active $(echo "$STATE2" | field "d['active']")"
echo "$STATE2" | field "[s['name'] for s in d['spaces']]" | grep -q only-on-a && fail "B's window lists none of A's spaces" || pass "B's window lists none of A's spaces"
[ -n "$W3" ] && [ "$(debug GET "/state?window=$W3" | field "d['window_server']")" = local ] && pass "window $W3 opens on A" || fail "a second window on A: '$W3'"
sleep 1

LA=$(windows_of "$T/a" "$AL"); LB=$(windows_of "$T/b" "$BL")
[ "$LA" = "1 $(printf '%s\n' "$W1" "$W3" | sort | tr '\n' ' ' | sed 's/ $//')" ] && pass "A lists one client holding $W1 and $W3" || fail "A's client ls: $LA"
[ "$LB" = "1 $W2" ] && pass "B lists one client holding $W2" || fail "B's client ls: $LB"

debug POST "/action?id=close-window&window=$W3" > /dev/null
for _ in $(seq 1 50); do [ "$(windows_of "$T/a" "$AL")" = "1 $W1" ] && break; sleep 0.2; done
LA=$(windows_of "$T/a" "$AL"); LB=$(windows_of "$T/b" "$BL")
[ "$LA" = "1 $W1" ] && pass "closing $W3 drops only its view from A" || fail "after closing $W3, A lists: $LA"
[ "$LB" = "1 $W2" ] && pass "B still lists $W2" || fail "after closing $W3, B lists: $LB"

[ -n "${SHOTS:-}" ] && mkdir -p "$SHOTS" && debug POST "/window/snapshot?window=$W2&path=$SHOTS/b-up.png" > /dev/null
kill "${PIDS[1]}" 2>/dev/null; wait "${PIDS[1]}" 2>/dev/null
for _ in $(seq 1 50); do debug GET "/state?window=$W2" | field "d['connection']" | grep -q unreachable && break; sleep 0.2; done
S1=$(debug GET "/state?window=$W1"); S2=$(debug GET "/state?window=$W2")
[ -n "${SHOTS:-}" ] && debug POST "/window/snapshot?window=$W2&path=$SHOTS/b-down.png" > /dev/null
echo "$S2" | field "d['connection']" | grep -q unreachable && [ "$(echo "$S2" | field "d['link_opacity']")" = 0.5 ] && pass "B's window reads Down at link_opacity 0.5" || fail "B's window: $(echo "$S2" | field "d['connection'], d['link_opacity']")"
[ "$(echo "$S1" | field "d['connection']")" = connected ] && [ "$(echo "$S1" | field "d['link_opacity']")" = 1.0 ] && pass "A's window stays connected at link_opacity 1.0" || fail "A's window: $(echo "$S1" | field "d['connection'], d['link_opacity']")"

[ "$FAILS" -eq 0 ] && echo "ALL PASS" || { echo "$FAILS FAILED"; exit 1; }
