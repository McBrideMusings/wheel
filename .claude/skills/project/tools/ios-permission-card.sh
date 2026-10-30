#!/usr/bin/env bash
# § iPhone permission card (wheel-h3z5.32, case 7): the phone's card shows the agent's reason, and
# its three buttons sit together at the right. A paired phone in the simulator attaches to an agent
# pane; the fake agent asks permission with a description; the phone logs the card it drew and
# `simctl io screenshot` saves what the screen shows (WHEEL_SHOTS=<dir>).
#
# Needs WHEEL_IOS_APP: the built Wheel.app for the simulator (WheelPhone scheme), and a pool simulator.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SIM=~/.claude/tools/sim
BUNDLE=com.piercemakes.wheel.phone
UDID=""
ios_cleanup() { [ -n "$UDID" ] && "$SIM" release >/dev/null 2>&1; cleanup; }
trap ios_cleanup EXIT

[ -d "${WHEEL_IOS_APP:-}" ] || { echo "set WHEEL_IOS_APP to the built Wheel.app (WheelPhone, iphonesimulator)" >&2; exit 2; }
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

mkdir -p "$SCR/proj"
A="$(w space new "$SCR/proj")"
P="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
w pane start "$P" agent >/dev/null

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
xcrun simctl install "$UDID" "$WHEEL_IOS_APP"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
PLOG="$DATA/Documents/phone.log"
rm -f "$PLOG"
plog() { grep -q -- "$1" "$PLOG" 2>/dev/null; }

code="$(w remote pair --address 127.0.0.1)"
SIMCTL_CHILD_WHEEL_PAIR_JSON="$code" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
check "the phone pairs" poll 40 plog "connected to wss://127.0.0.1:$RPORT/ws"

bg w agent send "$P" "permission described"
check "the request is waiting" poll 20 bash -c "[ -n \"\$($WHEEL --url ws://127.0.0.1:$PORT/ws --token-file $INST/token perm list | jq -r '.[0].id // empty')\" ]"
check "7: the phone's card shows the agent's reason" poll 30 plog 'permission card .*reason=Remove the build output so the next build starts clean'
check "7: the phone's card shows the decision reason and the path" poll 5 plog 'decision=Bash commands need approval in this mode path=/work/build'
sleep 2
xcrun simctl io "$UDID" screenshot "$SHOTS/permission-card-phone.png" >/dev/null 2>&1
echo "     shot $SHOTS/permission-card-phone.png"
[ "$FAIL" -eq 0 ] || { echo "--- phone.log"; tail -30 "$PLOG"; }
finish
