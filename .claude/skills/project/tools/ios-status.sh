#!/usr/bin/env bash
# § 7 Status dots on the phone: the iOS app draws the status the daemon sends in the carousel's top
# bar (the space's dot beside its name, the shown pane's dot beside its menu) and `status.dots`
# hides ranks. Installs the built WheelPhone into a pool simulator, pairs it with this script's
# own daemon and reads the app's phone.log. Shots (simctl io) go to $WHEEL_SHOTS (default:
# $SCR/shots), one per state and theme.
#
# Needs WHEEL_IOS_APP: the built Wheel.app for the simulator (WheelPhone scheme).
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

mkdir -p "$SCR/a"
A="$(w space new "$SCR/a")"
PA="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
w pane start "$PA" agent >/dev/null
bg w agent send "$PA" "permission please"

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
xcrun simctl install "$UDID" "$WHEEL_IOS_APP"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
PLOG="$DATA/Documents/phone.log"

launch() { # launch [pair]: (re)start the app; "pair" spends a fresh one-time token
  xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1
  if [ "${1:-}" = pair ]; then
    local code; code="$(w remote pair --address 127.0.0.1)"
    SIMCTL_CHILD_WHEEL_PAIR_JSON="$code" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
  else
    xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
  fi
}
plog() { grep -q -- "$1" "$PLOG" 2>/dev/null; }
wait_plog() { poll "${2:-30}" plog "$1"; }
shoot() { # shoot <name>: light and dark
  local theme
  for theme in light dark; do
    xcrun simctl ui "$UDID" appearance "$theme"
    sleep 1.5
    xcrun simctl io "$UDID" screenshot "$SHOTS/$1-$theme.png" >/dev/null 2>&1
    echo "shot $SHOTS/$1-$theme.png"
  done
}

launch pair
check "the phone pairs" wait_plog "paired with wss://127.0.0.1:$RPORT/ws" 40
check "the waiting pane is needs_you, as the first layout or a status_changed says" wait_plog "$A=Needs you, 1 waiting, $PA=Needs you, 1 waiting\|status $A/$PA: pane=Needs you, 1 waiting space=Needs you, 1 waiting" 40
sleep 2
shoot needs-you

PERM="$(perm_id)"
w perm answer "$PERM" allow >/dev/null
check "answering ends the turn: the pane is idle" wait_plog "status $A/$PA: pane=Idle" 40
sleep 1
shoot idle

bg w agent send "$PA" "work slowly 600"
check "a running turn is working" wait_plog "status $A/$PA: pane=Working" 40
sleep 1
shoot working

w config set status.dots needs_you,stopped --machine >/dev/null
check "status.dots = needs_you,stopped reaches the phone" wait_plog "status.dots = needs_you,stopped" 20
sleep 1
shoot working-hidden

w config unset status.dots --machine >/dev/null
check "unsetting it draws every rank again" wait_plog "status.dots = needs_you,stopped,working,idle" 20
finish
