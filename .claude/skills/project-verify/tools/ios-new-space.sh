#!/usr/bin/env bash
# § iOS New Space sheet (wheel-a10q.3): the phone lists the Mac's recent folders and roots, browses
# subfolders, searches with the palette's scorer and makes a space with space_create. Installs the
# built WheelPhone into a pool simulator, pairs it with this script's own daemon (whose home is a
# fixture) and reads the app's phone.log. The phone cannot tap under simctl, so the launch variable
# WHEEL_NEW_SPACE=<1|search:text|browse:path[,path]|create:path> drives the sheet. Shots go to
# $WHEEL_SHOTS (default $SCR/shots), light and dark.
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
FAKEHOME="$(cd "$SCR" && pwd -P)/home"
mkdir -p "$FAKEHOME/Projects/wheel/daemon" "$FAKEHOME/Projects/wharf" "$FAKEHOME/Projects/notes"
HOME="$FAKEHOME" start_daemon --state-file "$SCR/spaces.json"
w config set --machine spaces.roots "~/Projects" >/dev/null
w space new "$FAKEHOME/Projects/notes" >/dev/null

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
xcrun simctl install "$UDID" "$WHEEL_IOS_APP"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
PLOG="$DATA/Documents/phone.log"

launch() { # launch <WHEEL_NEW_SPACE value>: pairs on the first launch, reuses the credential after
  xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1
  if [ -z "${PAIRED:-}" ]; then
    local code; code="$(w remote pair --address 127.0.0.1)"
    SIMCTL_CHILD_WHEEL_NEW_SPACE="$1" SIMCTL_CHILD_WHEEL_PAIR_JSON="$code" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
    PAIRED=1
  else
    SIMCTL_CHILD_WHEEL_NEW_SPACE="$1" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
  fi
}
plog() { grep -q -- "$1" "$PLOG" 2>/dev/null; }
wait_plog() { poll "${2:-30}" plog "$1"; }
shoot() { # shoot <name>: light and dark
  local theme
  for theme in light dark; do
    xcrun simctl ui "$UDID" appearance "$theme"
    sleep 1.5
    xcrun simctl io "$UDID" screenshot "$SHOTS/new-space-$1-$theme.png" >/dev/null 2>&1
    echo "     shot $SHOTS/new-space-$1-$theme.png"
  done
}

launch 1
check "the phone pairs" wait_plog "paired with wss://127.0.0.1:$RPORT/ws" 40
check "the sheet asks for the roots" wait_plog "new space: roots \[\"$FAKEHOME/Projects\"\]" 40
check "and lists the root's three folders" wait_plog "new space: $FAKEHOME/Projects -> $FAKEHOME/Projects: 3 folders" 20
check "and the recent directory" wait_plog "new space: 1 recent directories listed" 20
sleep 1
shoot roots

launch "browse:$FAKEHOME/Projects,$FAKEHOME/Projects/wheel"
check "browsing wheel asks the daemon for its subfolders" wait_plog "new space: $FAKEHOME/Projects/wheel -> $FAKEHOME/Projects/wheel: 1 folders" 40
sleep 1
shoot subfolders

launch "search:whe"
check "searching whe lists wheel, with the palette's scorer" wait_plog 'new space search "whe": .*wheel' 40
sleep 1
shoot search

launch "create:$FAKEHOME/Projects/wheel"
check "Create space here makes the space" poll 30 sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' space ls | grep -q 'Projects/wheel'"
check "the daemon logged the space" grep -q "space created.*Projects/wheel" "$(LOG)"
finish
