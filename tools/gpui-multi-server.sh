#!/bin/bash
# Check wheel-2aln.8 (ADR-0057): the GPUI Mac app keeps one connection per wheeld, shared by its
# windows, and pairs with a remote wheeld. Two throwaway wheelds: A, the app's local one, and B with
# its remote listener on 127.0.0.1. The hidden app (never shown, never focused) is driven through
# its debug server: it pairs with B's code (POST /servers/pair), opens a window on B
# (POST /servers/window) and a second window on A. Then:
#   - A's `wheel client ls` lists one client holding both A windows, B's lists one holding B's;
#   - closing one A window drops only its view from A's list and leaves B's alone;
#   - killing B leaves only B's window Down, drawn at link_opacity 0.5, and A's windows connected.
#   - (wheel-2aln.18) relaunched with B's credential gone from the Keychain, the app leaves B's
#     saved window closed and keeps its record in windows.json; relaunched with B unpaired, it
#     drops the record.
#   - the session menu (wheel-2aln.9): with one server today's rows then Add Server… and no mark;
#     with two a heading per server, the session button's mark and Settings naming the server;
#     pressing B's session moves the window to B (B lists it, A drops it, windows.json names B);
#     New Session on this Mac from that window makes the session on A and moves the window back;
#     Orchestrator mode moves a window to the server whose orchestrator folder has a space.
# The server list goes to a scratch file (WHEEL_GPUI_SERVERS_FILE); the credential goes to the
# login Keychain under "Wheel App Server" and B's id, and the cleanup deletes it. Prints PASS or
# FAIL per check; kills what it starts by pid.
#
# SHOTS=<dir> also draws B's window to <dir>/b-up.png before B is killed and b-down.png after, and
# the first window's session menu with one server and with two, and its Settings with two.
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

# launch_app <log> [args…]: the hidden app on A, its pid in APP_PID, waited on until its debug server answers.
launch_app() {
  local log=$1; shift
  WHEEL_HIDDEN=1 WHEEL_DEBUG_PORT=$DEBUG WHEEL_URL="ws://127.0.0.1:$AL/ws" WHEEL_TOKEN_FILE="$T/a/support/token" \
    WHEEL_GPUI_SERVERS_FILE="$T/servers.json" WHEEL_GPUI_WINDOWS_FILE="$T/windows.json" \
    WHEEL_GPUI_BRIDGE="$APP/Contents/MacOS/libWheelGpuiBridge.dylib" \
    "$APP/Contents/MacOS/wheel-gpui" "$@" > "$log" 2>&1 &
  APP_PID=$!
  PIDS+=($APP_PID)
  for _ in $(seq 1 100); do debug GET /state > /dev/null 2>&1 && break; sleep 0.2; done
}
launch_app "$T/app.out" --space "$SA"
W1=$(debug GET /state | field "d['window']")

# One server: today's rows plus Add Server…, no heading, the icon-only session button.
debug POST "/titlemenu?window=$W1&action=open&menu=session" > /dev/null
M1=$(debug GET "/state?window=$W1")
[ "$(echo "$M1" | field "[r['id'] for r in d['title']['menu']['rows']][-4:]")" = "['new-session', 'rename-session', 'close-session', 'add-server']" ] && [ "$(echo "$M1" | field "any(r['heading'] for r in d['title']['menu']['rows'])")" = False ] && pass "one server: the session menu reads as before, then Add Server…" || fail "one-server menu: $(echo "$M1" | field "d['title']['menu']")"
[ "$(echo "$M1" | field "[c['icon'] for c in d['title']['row'] if c['id'] == 'session']")" = "['rectangle.stack']" ] && pass "one server: the session button has no server mark" || fail "one-server session button: $(echo "$M1" | field "[c for c in d['title']['row'] if c['id'] == 'session']")"
[ -n "${SHOTS:-}" ] && mkdir -p "$SHOTS" && debug POST "/window/snapshot?window=$W1&path=$SHOTS/menu-one-server.png" > /dev/null
debug POST "/titlemenu?window=$W1&action=close" > /dev/null

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

# Two servers: the session menu groups by server (wheel-2aln.9).
for _ in $(seq 1 50); do [ "$(debug GET "/state?window=$W1" | field "sum(1 for g in d['directory'] if g['up'])")" = 2 ] && break; sleep 0.2; done
debug POST "/titlemenu?window=$W1&action=open&menu=session" > /dev/null
M2=$(debug GET "/state?window=$W1")
[ "$(echo "$M2" | field "[r['id'] for r in d['title']['menu']['rows'] if r['heading']]")" = "['server:local', 'server:$B_ID']" ] && pass "two servers: a heading per server, this Mac first" || fail "two-server headings: $(echo "$M2" | field "[r['id'] for r in d['title']['menu']['rows']]")"
B_ROW=$(echo "$M2" | field "next(r['id'] for r in d['title']['menu']['rows'] if r['id'].startswith('session:$B_ID:'))")
[ -n "$B_ROW" ] && pass "B's sessions are listed under its heading ($B_ROW)" || fail "no session row of B"
[ "$(echo "$M2" | field "[c['icon'] for c in d['title']['row'] if c['id'] == 'session']")" = "['desktopcomputer']" ] && pass "two servers: the session button carries this Mac's mark" || fail "two-server session button: $(echo "$M2" | field "[c for c in d['title']['row'] if c['id'] == 'session']")"
[ -n "${SHOTS:-}" ] && debug POST "/window/snapshot?window=$W1&path=$SHOTS/menu-two-servers.png" > /dev/null
A_NAME=$(echo "$M2" | field "d['directory'][0]['name']")
debug POST "/titlemenu?window=$W1&action=close" > /dev/null
debug POST "/title?window=$W1&id=settings" > /dev/null
[ "$(debug GET "/state?window=$W1" | field "d['settings']['title']")" = "Settings · $A_NAME" ] && pass "Settings names the window's server ($A_NAME)" || fail "settings title: $(debug GET "/state?window=$W1" | field "d['settings']")"
[ -n "${SHOTS:-}" ] && debug POST "/window/snapshot?window=$W1&path=$SHOTS/settings-two-servers.png" > /dev/null
debug POST "/title?window=$W1&id=settings" > /dev/null
debug POST "/rename?window=$W1&kind=server&action=begin" > /dev/null
[ "$(debug GET "/state?window=$W1" | field "d['window_info']['name_edit']['target']")" = server ] && pass "Add Server… opens its card" || fail "Add Server card: $(debug GET "/state?window=$W1" | field "d['window_info']['name_edit']")"
[ -n "${SHOTS:-}" ] && debug POST "/window/snapshot?window=$W1&path=$SHOTS/add-server.png" > /dev/null
debug POST "/rename?window=$W1&kind=server&action=cancel" > /dev/null

# Pressing B's session moves W1 to B.
debug POST "/titlemenu?window=$W1&action=open&menu=session" > /dev/null
debug POST "/titlemenu?window=$W1&action=select&item=$B_ROW" > /dev/null
for _ in $(seq 1 50); do [ "$(debug GET "/state?window=$W1" | field "d['window_server']")" = "$B_ID" ] && [ "$(debug GET "/state?window=$W1" | field "d['active']")" = "$SB" ] && break; sleep 0.2; done
S1B=$(debug GET "/state?window=$W1")
[ "$(echo "$S1B" | field "d['window_server']")" = "$B_ID" ] && [ "$(echo "$S1B" | field "d['active']")" = "$SB" ] && [ "$(echo "$S1B" | field "d['connection']")" = connected ] && pass "pressing B's session shows B's space $SB in $W1" || fail "after B's session row, $W1: $(echo "$S1B" | field "d['window_server'], d['active'], d['connection']")"
for _ in $(seq 1 50); do [ "$(windows_of "$T/b" "$BL")" = "1 $(printf '%s\n' "$W1" "$W2" | sort | tr '\n' ' ' | sed 's/ $//')" ] && break; sleep 0.2; done
windows_of "$T/a" "$AL" | grep -q "$W1" && fail "A still lists $W1: $(windows_of "$T/a" "$AL")" || pass "A no longer lists $W1"
[ "$(windows_of "$T/b" "$BL")" = "1 $(printf '%s\n' "$W1" "$W2" | sort | tr '\n' ' ' | sed 's/ $//')" ] && pass "B lists $W1 beside $W2" || fail "B's client ls: $(windows_of "$T/b" "$BL")"
# A saved record names no window id: with W1 moved, both main windows' records name B.
on_b_saved() { python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(sum(1 for w in d.get('windows', []) if w.get('server') == sys.argv[2]))" "$T/windows.json" "$B_ID" 2>/dev/null; }
for _ in $(seq 1 50); do [ "$(on_b_saved)" = 2 ] && break; sleep 0.2; done
[ "$(on_b_saved)" = 2 ] && pass "windows.json saves $W1's record on B beside $W2's" || fail "windows.json: $(cat "$T/windows.json" 2>&1 | head -c 600)"

# New Session on this Mac, asked from W1 while it shows B: A makes it and W1 moves back to show it.
count() { on "$1" "$2" session ls --json | python3 -c 'import json,sys; d=json.load(sys.stdin); print(len(d if isinstance(d, list) else d.get("sessions", [])))'; }
NA=$(count "$T/a" "$AL")
debug POST "/titlemenu?window=$W1&action=open&menu=session" > /dev/null
debug POST "/titlemenu?window=$W1&action=select&item=new-session:local" > /dev/null
for _ in $(seq 1 50); do [ "$(count "$T/a" "$AL")" -gt "$NA" ] && [ "$(debug GET "/state?window=$W1" | field "d['window_server']")" = local ] && break; sleep 0.2; done
NEW=$(on "$T/a" "$AL" session ls --json | python3 -c 'import json,sys; d=json.load(sys.stdin); d=d if isinstance(d, list) else d.get("sessions", []); print(d[-1]["id"])')
[ "$(count "$T/a" "$AL")" -gt "$NA" ] && pass "New Session on this Mac makes a session on A ($NEW)" || fail "A's sessions stayed at $NA"
S1A=$(debug GET "/state?window=$W1")
[ "$(echo "$S1A" | field "d['window_server']")" = local ] && [ "$(echo "$S1A" | field "d['session']")" = "$NEW" ] && pass "$W1 moves back to A and shows the new session" || fail "after New Session on A, $W1: $(echo "$S1A" | field "d['window_server'], d['session']")"

# Orchestrator mode follows the one orchestrator: A has no space on its orchestrator folder, B
# opens one on its own (the default, orchestrator/ beside its machine file), so W1 moves to B.
mkdir -p "$T/b/orchestrator"
SO=$(on "$T/b" "$BL" space new "$T/b/orchestrator" --name orch-on-b)
debug POST "/titlemenu?window=$W1&action=open&menu=mode" > /dev/null
debug POST "/titlemenu?window=$W1&action=select&item=mode:orchestrator" > /dev/null
for _ in $(seq 1 50); do [ "$(debug GET "/state?window=$W1" | field "d['window_server']")" = "$B_ID" ] && [ "$(debug GET "/state?window=$W1" | field "d['active']")" = "$SO" ] && break; sleep 0.2; done
SOR=$(debug GET "/state?window=$W1")
[ "$(echo "$SOR" | field "d['window_server']")" = "$B_ID" ] && [ "$(echo "$SOR" | field "d['active']")" = "$SO" ] && pass "Orchestrator mode moves $W1 to B's orchestrator space $SO" || fail "orchestrator follow, $W1: $(echo "$SOR" | field "d['window_server'], d['active'], d['title']['mode']")"
debug POST "/titlemenu?window=$W1&action=open&menu=mode" > /dev/null
debug POST "/titlemenu?window=$W1&action=select&item=mode:spaces" > /dev/null
debug POST "/titlemenu?window=$W1&action=open&menu=session" > /dev/null
debug POST "/titlemenu?window=$W1&action=select&item=session:local:$NEW" > /dev/null
for _ in $(seq 1 50); do [ "$(debug GET "/state?window=$W1" | field "d['window_server']")" = local ] && [ "$(debug GET "/state?window=$W1" | field "d['connection']")" = connected ] && break; sleep 0.2; done
[ "$(debug GET "/state?window=$W1" | field "d['window_server']")" = local ] && pass "$W1 goes back to A from the menu" || fail "back to A: $(debug GET "/state?window=$W1" | field "d['window_server'], d['session']")"

[ -n "${SHOTS:-}" ] && mkdir -p "$SHOTS" && debug POST "/window/snapshot?window=$W2&path=$SHOTS/b-up.png" > /dev/null
kill "${PIDS[1]}" 2>/dev/null; wait "${PIDS[1]}" 2>/dev/null
for _ in $(seq 1 50); do debug GET "/state?window=$W2" | field "d['connection']" | grep -q unreachable && break; sleep 0.2; done
S1=$(debug GET "/state?window=$W1"); S2=$(debug GET "/state?window=$W2")
[ -n "${SHOTS:-}" ] && debug POST "/window/snapshot?window=$W2&path=$SHOTS/b-down.png" > /dev/null
echo "$S2" | field "d['connection']" | grep -q unreachable && [ "$(echo "$S2" | field "d['link_opacity']")" = 0.5 ] && pass "B's window reads Down at link_opacity 0.5" || fail "B's window: $(echo "$S2" | field "d['connection'], d['link_opacity']")"
[ "$(echo "$S1" | field "d['connection']")" = connected ] && [ "$(echo "$S1" | field "d['link_opacity']")" = 1.0 ] && pass "A's window stays connected at link_opacity 1.0" || fail "A's window: $(echo "$S1" | field "d['connection'], d['link_opacity']")"

# Relaunch with B's credential unreadable: B's saved window stays closed, and its record stays in
# windows.json through the writes the relaunch makes. Then unpaired: the record goes.
# saved_on_b: how many of windows.json's main windows name B.
saved_on_b() { python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(sum(1 for w in d.get('windows', []) if w.get('server') == sys.argv[2]))" "$T/windows.json" "$B_ID" 2>/dev/null; }
BEFORE=$(saved_on_b)
kill "$APP_PID" 2>/dev/null; wait "$APP_PID" 2>/dev/null
security delete-generic-password -s "Wheel App Server" -a "$B_ID" > /dev/null 2>&1
launch_app "$T/app-no-credential.out"
debug POST "/action?id=new-window" > /dev/null
for _ in $(seq 1 50); do [ "$(debug GET /state | field "len(d['window_info']['windows']['windows'])")" -ge 2 ] && break; sleep 0.2; done
sleep 1
grep -q "a saved window on $B_ID stays closed" "$T/app-no-credential.out" && [ "$(debug GET /state | field "sum(1 for w in d['window_info']['windows']['windows'] if w.get('server') == '$B_ID')")" = 0 ] && pass "with B's credential unreadable, B's saved window stays closed" || fail "no-credential launch: $(grep 'saved window' "$T/app-no-credential.out"), windows $(debug GET /state | field "d['window_info']['windows']['windows']")"
[ "$BEFORE" -ge 1 ] && [ "$(saved_on_b)" = "$BEFORE" ] && pass "windows.json keeps B's $BEFORE record(s) through the relaunch's writes" || fail "B's records went from $BEFORE to $(saved_on_b): $(head -c 600 "$T/windows.json")"
kill "$APP_PID" 2>/dev/null; wait "$APP_PID" 2>/dev/null
echo '[]' > "$T/servers.json"
launch_app "$T/app-unpaired.out"
debug POST "/action?id=new-window" > /dev/null
sleep 2
grep -q "a saved window on $B_ID is dropped" "$T/app-unpaired.out" && [ "$(saved_on_b)" = 0 ] && pass "with B unpaired, windows.json drops B's records" || fail "unpaired launch: $(grep 'saved window' "$T/app-unpaired.out"), B records $(saved_on_b)"

[ "$FAILS" -eq 0 ] && echo "ALL PASS" || { echo "$FAILS FAILED"; exit 1; }
