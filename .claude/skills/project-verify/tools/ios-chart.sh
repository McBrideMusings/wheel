#!/usr/bin/env bash
# § iPhone chart blocks (wheel-eg4e.19.12): an agent reply holding a ```chart block with two series
# and one whose mark is not a chart's. The phone draws the first with Swift Charts and logs
# `chart in <agent>: bar, 5 rows`, and keeps the second as code with its reason under it, logging
# `chart in <agent> could not draw: <reason>`. `simctl io screenshot` saves the chat
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

mkdir -p "$SCR/work"
WORK="$(w space new "$SCR/work")"
w space rename "$WORK" work >/dev/null
P1="$(w space ls | jq -r --arg s "$WORK" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
w pane start "$P1" agent >/dev/null
AGENT="$(w space ls | jq -r --arg p "$P1" '[.spaces[].root | .. | select(.id? == $p)][0].agent')"
check "wheeld started the agent pane" test -n "$AGENT" -a "$AGENT" != null

REPLY='Builds this week:

```chart
{"mark": "bar", "data": [{"x": "Mon", "y": 3, "series": "green"}, {"x": "Mon", "y": 1, "series": "red"}, {"x": "Tue", "y": 5, "series": "green"}, {"x": "Wed", "y": 2, "series": "green"}, {"x": "Wed", "y": 2, "series": "red"}]}
```

And a pie:

```chart
{"mark": "pie", "data": []}
```
'
check "the agent echoes the reply with both chart blocks" w agent send "$P1" "$REPLY" --timeout 30

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
xcrun simctl install "$UDID" "$WHEEL_IOS_APP"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
PLOG="$DATA/Documents/phone.log"
rm -f "$PLOG"
plog() { grep -qF -- "$1" "$PLOG" 2>/dev/null; }

code="$(w remote pair --address 127.0.0.1)"
env SIMCTL_CHILD_WHEEL_PAIR_JSON="$code" SIMCTL_CHILD_WHEEL_START_SPACE=work xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
check "the phone pairs" poll 40 plog "connected to wss://127.0.0.1:$RPORT/ws"
check "case 2: the phone draws the bar chart with its five rows" poll 30 plog "chart in $AGENT: bar, 5 rows"
check "case 3: the pie block stays code with its reason" poll 30 plog "chart in $AGENT could not draw: mark \"pie\" is not bar, line, point or area"
sleep 3
xcrun simctl io "$UDID" screenshot "$SHOTS/ios-chart.png" >/dev/null 2>&1

[ "$FAIL" -eq 0 ] || { echo "--- phone.log"; grep -E "chart|code block|connected|page" "$PLOG" | tail -30; }
echo "shots: $SHOTS"
finish
