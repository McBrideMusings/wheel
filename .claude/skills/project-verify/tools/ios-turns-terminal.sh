#!/usr/bin/env bash
# § iOS pane that turns terminal (wheel-cxf6): a runner splits a web pane (a new empty pane), then
# turns the new pane into a terminal in a second broadcast with the same pane ids. The carousel
# never re-runs show(pane:), so PhoneModel re-resolves its terminal on every layout; without that the
# phone never sends term_attach and the page stays on "Attaching…".
#
# Needs WHEEL_IOS_APP: the built Wheel.app for the simulator (WheelPhone scheme), and a pool simulator.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SIM=~/.claude/tools/sim
BUNDLE=com.piercemakes.wheel.phone
UDID=""
ios_cleanup() { [ -n "$UDID" ] && "$SIM" release >/dev/null 2>&1; cleanup; }
trap ios_cleanup EXIT

[ -d "${WHEEL_IOS_APP:-}" ] || { echo "set WHEEL_IOS_APP to the built Wheel.app (WheelPhone, iphonesimulator)" >&2; exit 2; }

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

SITE="$SCR/proj"
mkdir -p "$SITE/site"
printf '<!doctype html><title>t</title><body>turns-terminal</body>\n' >"$SITE/site/index.html"
printf '[artifacts.site]\npath = "site"\n' >"$SITE/wheel.toml"
w space new "$SITE" >/dev/null
WEB="$(w space ls --json | jget 0.panes.0)"
w pane kind "$WEB" web --artifact site >/dev/null

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
check "the web pane's page loads" poll 30 plog "web pane $WEB loaded, status 200"

NEW="$(w pane split "$WEB" right)"
check "the split made a new pane" test -n "$NEW"
check "the phone shows the new empty pane" poll 20 plog "layout: .*$NEW"
w pane kind "$NEW" terminal >/dev/null
check "the phone attaches the pane that turned terminal" poll 30 grep -Eq "attaching to " "$PLOG"
TERM_ID="$(grep -o 'attaching to [^ ,]*' "$PLOG" | head -1 | sed 's/attaching to //')"
check "it attached to a terminal id" test -n "$TERM_ID"
check "the phone is attached, replay received" poll 30 plog "attached to $TERM_ID"
[ "$FAIL" -eq 0 ] || { echo "--- phone.log"; tail -40 "$PLOG"; echo "--- wheeld.log"; grep -E "term|attach" "$(LOG)" | tail -20 | cut -c1-300; }
finish
