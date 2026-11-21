#!/usr/bin/env bash
# § iPhone mirror pane (wheel-eg4e.19.7): the phone draws a mirror as its source. Space `work`
# holds a terminal named `build`, an agent pane and a web pane; space `orch` mirrors each. The
# phone shows `orch` and, on each mirror's page, attaches to the source: text typed into the
# terminal mirror reaches the source's shell, a message sent from the agent mirror reaches the
# source's agent, and the web mirror loads the source's page. `simctl io screenshot` saves each page
# (WHEEL_SHOTS=<dir>).
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

mkdir -p "$SCR/work/site" "$SCR/orch"
printf '<!doctype html><title>t</title><body>mirrored-page</body>\n' >"$SCR/work/site/index.html"
printf '[artifacts.site]\npath = "site"\n' >"$SCR/work/wheel.toml"
first_pane() { w space ls | jq -r --arg s "$1" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id'; }
WORK="$(w space new "$SCR/work")"
w space rename "$WORK" work >/dev/null
P1="$(first_pane "$WORK")"
w pane start "$P1" terminal >/dev/null
w pane rename "$P1" build >/dev/null
P2="$(w pane split "$P1" right)"
w pane start "$P2" agent >/dev/null
P3="$(w pane split "$P2" down)"
w pane kind "$P3" web --artifact site >/dev/null
TERM_ID="$(w space ls | jq -r --arg p "$P1" '[.spaces[].root | .. | select(.id? == $p)][0].term')"
ORCH="$(w space new "$SCR/orch")"
w space rename "$ORCH" orch >/dev/null
M1="$(w pane mirror "$P1" --space "$ORCH")"
M2="$(w pane mirror "$P2" --space "$ORCH")"
M3="$(w pane mirror "$P3" --space "$ORCH")"
check "wheeld made three mirrors" test -n "$M1" -a -n "$M2" -a -n "$M3"

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
xcrun simctl install "$UDID" "$WHEEL_IOS_APP"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
PLOG="$DATA/Documents/phone.log"
rm -f "$PLOG"
plog() { grep -q -- "$1" "$PLOG" 2>/dev/null; }
# Orch's pages in the phone's reading order: the three mirrors, M1 to M3.
orch_pages() { grep -qF "active $ORCH, panes [\"$M1\", \"$M2\", \"$M3\"]" "$PLOG"; }
launch() { xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1; sleep 1; env "$@" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null; }
shot() { sleep 3; xcrun simctl io "$UDID" screenshot "$SHOTS/ios-mirror-$1.png" >/dev/null 2>&1; }

code="$(w remote pair --address 127.0.0.1)"
launch SIMCTL_CHILD_WHEEL_PAIR_JSON="$code" SIMCTL_CHILD_WHEEL_START_SPACE=orch
check "the phone pairs" poll 40 plog "connected to wss://127.0.0.1:$RPORT/ws"
check "case 1/4: the terminal mirror draws as its source, titled work · build" poll 20 plog "mirror $M1 in $ORCH shows $P1 as terminal: work · build"
check "case 2/4: the agent mirror draws as its source's agent" poll 20 plog "mirror $M2 in $ORCH shows $P2 as agent: work · "
check "case 3/4: the web mirror draws as its source's page" poll 20 plog "mirror $M3 in $ORCH shows $P3 as web: work · "

# From here on the phone shows orch alone, so an attach in the log is the mirror's.
xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1; : >"$PLOG"
launch SIMCTL_CHILD_WHEEL_START_SPACE=orch SIMCTL_CHILD_WHEEL_START_PAGE=1 SIMCTL_CHILD_WHEEL_TYPE='echo mirror-$((6*7))'
check "case 1: the terminal mirror attaches to the source's terminal" poll 30 plog "attached to $TERM_ID"
check "case 1: text typed in the mirror reaches the source's shell" w term read "$TERM_ID" --until mirror-42 --timeout 20
check "the phone shows the terminal mirror's page" poll 30 plog "page $M1:"
shot terminal

launch SIMCTL_CHILD_WHEEL_START_SPACE=orch SIMCTL_CHILD_WHEEL_START_PAGE=2 SIMCTL_CHILD_WHEEL_SEND=hello-mirror
check "case 2: a message sent from the mirror reaches the source's agent" poll 30 sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' agent read '$P2' | grep -q 'echo: hello-mirror'"
check "the phone shows the agent mirror's page" poll 30 plog "page $M2:"
shot agent

launch SIMCTL_CHILD_WHEEL_START_SPACE=orch SIMCTL_CHILD_WHEEL_START_PAGE=3
check "case 3: the web mirror loads the source's page" poll 30 plog "web pane $M3 loaded, status 200"
check "the phone shows the web mirror's page" poll 30 plog "page $M3:"
shot web
check "the phone's pages in orch are the three mirrors, in order" orch_pages

[ "$FAIL" -eq 0 ] || { echo "--- phone.log"; grep -E "mirror|layout|attach|web pane|send to|typed" "$PLOG" | tail -30; }
echo "shots: $SHOTS"
finish
