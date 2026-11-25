#!/bin/bash
# Check the iPhone app with two paired wheelds (wheel-2aln.10, ADR-0057): two throwaway wheelds
# ("alpha" teal, "beta" orange) with remote listeners on loopback; one launch pastes both pairing
# codes; the Keychain keeps both and a relaunch reconnects to both; a tap on beta's session chip
# shows it, and wheel client ls lists the phone on beta alone; with beta stopped its chips dim.
# Keeps screenshots in $SHOTS (default $T/shots). Uses no GUI of the Mac; kills what it starts by pid.
#
#   WHEEL_IOS_APP=<built WheelPhone Wheel.app> UDID=<booted simulator> tools/ios-servers.sh
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
APP=${WHEEL_IOS_APP:?set WHEEL_IOS_APP to the built WheelPhone Wheel.app}
UDID=${UDID:?set UDID to a booted simulator}
BUNDLE=com.piercemakes.wheel.phone
T=/private/tmp/claude/wheel/ios-servers
SHOTS=${SHOTS:-$T/shots}
FAILS=0
PIDS=()
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
stop_all() { for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done; xcrun simctl terminate "$UDID" "$BUNDLE" > /dev/null 2>&1; }
trap stop_all EXIT

# start <name> <colour>: a wheeld under $T/<name>, its CLI port in PORT_<name>, its pid in PID_<name>.
start() {
  local name=$1 color=$2 port remote
  port=$(free_port); remote=$(free_port)
  mkdir -p "$T/$name/support" "$T/$name/logs"
  [ -f "$T/$name/machine.toml" ] || printf '[remote]\nlisten = "127.0.0.1:%s"\n[server]\nname = "%s"\ncolor = "%s"\n' "$remote" "$name" "$color" > "$T/$name/machine.toml"
  "$WHEELD" --listen "127.0.0.1:$port" --support-dir "$T/$name/support" --machine-file "$T/$name/machine.toml" \
    --log-dir "$T/$name/logs" --cli "$WHEEL" >> "$T/$name/wheeld.out" 2>&1 &
  local pid=$!
  PIDS+=("$pid")
  eval "PORT_$name=$port PID_$name=$pid"
  for _ in $(seq 1 100); do
    [ -f "$T/$name/support/token" ] && nc -z 127.0.0.1 "$port" > /dev/null 2>&1 && return
    sleep 0.2
  done
}
w() { local name=$1; shift; local port; eval "port=\$PORT_$name"
  env -u WHEEL_PANE -u WHEEL_SPACE "$WHEEL" --url "ws://127.0.0.1:$port/ws" --token-file "$T/$name/support/token" "$@"; }
LOG() { echo "$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)/Documents/phone.log"; }
launch() { xcrun simctl terminate "$UDID" "$BUNDLE" > /dev/null 2>&1; : > "$(LOG)" 2>/dev/null
  env SIMCTL_CHILD_WHEEL_NOTIFICATIONS=off "$@" xcrun simctl launch "$UDID" "$BUNDLE" > /dev/null; }
wait_log() { for _ in $(seq 1 60); do grep -q "$1" "$(LOG)" 2>/dev/null && return 0; sleep 0.5; done; return 1; }
phone_view() { w "$1" client ls --json | python3 -c '
import json, sys
print(" ".join(win.get("session") or "-" for c in json.load(sys.stdin) for win in c.get("windows", []) if win.get("window") == "phone"))'; }

rm -rf "$T"; mkdir -p "$SHOTS"
start alpha teal
start beta orange
for n in alpha beta; do w "$n" space new "$T" --name "$n-space" > /dev/null; done
WORK=$(w beta session new work)
w beta space new "$T" --name beta-work --session "$WORK" > /dev/null

xcrun simctl uninstall "$UDID" "$BUNDLE" > /dev/null 2>&1
xcrun simctl keychain "$UDID" reset > /dev/null 2>&1
xcrun simctl install "$UDID" "$APP"
CODES="[$(w alpha remote pair --address 127.0.0.1),$(w beta remote pair --address 127.0.0.1)]"
launch SIMCTL_CHILD_WHEEL_PAIR_JSON="$CODES" SIMCTL_CHILD_WHEEL_SESSIONS_OPEN=1
wait_log "server alpha id" && wait_log "server beta id" && pass "both codes pair (phone.log names servers alpha and beta)" || fail "both codes pair"
[ "$(w alpha remote devices | grep -c connected)" -ge 1 ] && [ "$(w beta remote devices | grep -c connected)" -ge 1 ] \
  && pass "each wheeld lists the phone as a connected device" || fail "each wheeld lists the phone"
sleep 3
xcrun simctl io "$UDID" screenshot "$SHOTS/two-servers.png" > /dev/null 2>&1

launch SIMCTL_CHILD_WHEEL_SHOW_SESSION=alpha/Main SIMCTL_CHILD_WHEEL_SESSIONS_OPEN=1
wait_log "servers: 2 paired" && pass "a relaunch reads two pairings from the Keychain" || fail "a relaunch reads two pairings (log: $(grep 'servers:' "$(LOG)" | tail -1))"
wait_log "showing server alpha" && pass "a tap on alpha's Main chip shows alpha" || fail "a tap on alpha's Main chip shows alpha"
sleep 3
[ -n "$(phone_view alpha)" ] && [ -z "$(phone_view beta)" ] && pass "client ls: the phone on alpha ($(phone_view alpha)), none on beta" \
  || fail "client ls after alpha (alpha '$(phone_view alpha)', beta '$(phone_view beta)')"
xcrun simctl io "$UDID" screenshot "$SHOTS/alpha-shown.png" > /dev/null 2>&1

launch SIMCTL_CHILD_WHEEL_SHOW_SESSION=beta/work SIMCTL_CHILD_WHEEL_SESSIONS_OPEN=1
wait_log "showing server beta" && wait_log "switch to session $WORK" && pass "a tap on beta's work chip shows beta's session work" || fail "a tap on beta's work chip"
sleep 3
[ "$(phone_view beta)" = "$WORK" ] && [ -z "$(phone_view alpha)" ] && pass "client ls: the phone on beta's work ($WORK), none on alpha" \
  || fail "client ls after beta (alpha '$(phone_view alpha)', beta '$(phone_view beta)')"
xcrun simctl io "$UDID" screenshot "$SHOTS/beta-shown.png" > /dev/null 2>&1

kill "$PID_beta"; sleep 4
xcrun simctl io "$UDID" screenshot "$SHOTS/beta-down.png" > /dev/null 2>&1

[ "$FAILS" -eq 0 ] && echo "ALL PASS" || { echo "$FAILS FAILED"; exit 1; }
