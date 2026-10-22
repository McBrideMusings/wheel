#!/usr/bin/env bash
# § iOS app shell: the phone pairs with a one-time token, keeps its own credential, reconnects
# after a relaunch with no code, and is refused once the Mac revokes it. Installs the built
# WheelPhone into a pool simulator, pairs it with this script's own daemon and reads the app's
# phone.log and the daemon's log.
#
# Needs WHEEL_IOS_APP: the built Wheel.app for the simulator (WheelPhone scheme).
# WHEEL_IOS_HOST (default 127.0.0.1) is the address the phone dials; a non-loopback one (the Mac's
# LAN or Tailscale address) makes the listener bind 0.0.0.0 and checks the pin over a connection
# App Transport Security would refuse.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SIM=~/.claude/tools/sim
BUNDLE=com.piercemakes.wheel.phone
HOST="${WHEEL_IOS_HOST:-127.0.0.1}"
BIND=127.0.0.1; [ "$HOST" = 127.0.0.1 ] || BIND=0.0.0.0
UDID=""
ios_cleanup() { [ -n "$UDID" ] && "$SIM" release >/dev/null 2>&1; cleanup; }
trap ios_cleanup EXIT

[ -d "${WHEEL_IOS_APP:-}" ] || { echo "set WHEEL_IOS_APP to the built Wheel.app (WheelPhone, iphonesimulator)" >&2; exit 2; }

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "%s:%s"\n' "$BIND" "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"
mkdir -p "$SCR/a"
w space new "$SCR/a" >/dev/null

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
xcrun simctl install "$UDID" "$WHEEL_IOS_APP"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
PLOG="$DATA/Documents/phone.log"
rm -f "$PLOG"

stop_app() { xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1; }
plog() { grep -q -- "$1" "$PLOG" 2>/dev/null; }
wait_plog() { poll "${2:-30}" plog "$1"; }
dlog() { grep -q -- "$1" "$(LOG)" 2>/dev/null; }
wait_dlog() { poll "${2:-30}" dlog "$1"; }
# Lines of phone.log written after the app was last started.
lines_since() { tail -n +"$(( $1 + 1 ))" "$PLOG"; }
phone_lines() { wc -l <"$PLOG" 2>/dev/null || echo 0; }

# 1. pair with the one-time token
code="$(w remote pair --address "$HOST")"
SIMCTL_CHILD_WHEEL_PAIR_JSON="$code" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
check "the phone pairs" wait_plog "paired with wss://$HOST:$RPORT/ws" 40
check "the phone holds the credential the daemon issued" wait_plog "device d1 paired; holding its credential"
check "the daemon lists the device connected" poll 10 sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' remote devices | grep -q '^d1 .*connected'"
check "the daemon log says it paired a device" dlog "paired a device via=.* device=d1"

# 2. relaunch with no code
stop_app
M="$(phone_lines)"
xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
check "a relaunch with no code reconnects" poll 30 sh -c "tail -n +$((M + 1)) '$PLOG' | grep -q 'connected to wss://$HOST:$RPORT/ws'"
check "the relaunch did not spend a pairing code" sh -c "! tail -n +$((M + 1)) '$PLOG' | grep -q 'pairing from WHEEL_PAIR_JSON'"
check "the relaunch shows the layout" sh -c "tail -n +$((M + 1)) '$PLOG' | grep -q 'layout: 1 spaces'"
check "the daemon did not issue a second pairing" sh -c "[ \"\$(grep -c 'paired a device' '$(LOG)')\" = 1 ]"

# 3. revoke
w remote revoke d1 >/dev/null
check "the daemon closes the revoked device's connection" wait_dlog "device revoked; closing its connection .*caller=Device(\"d1\")" 10
stop_app
M="$(phone_lines)"
xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
check "a relaunch after revoke is refused" wait_dlog "refused: missing or wrong token via=.*listener=Remote" 20
check "the phone reports the daemon unreachable" poll 20 sh -c "tail -n +$((M + 1)) '$PLOG' | grep -q 'daemon unreachable'"
check "the phone does not reach the layout" sh -c "! tail -n +$((M + 1)) '$PLOG' | grep -q 'layout:'"
check "the revoked device is gone from the daemon" sh -c "! '$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' remote devices | grep -q '^d1 '"
finish
