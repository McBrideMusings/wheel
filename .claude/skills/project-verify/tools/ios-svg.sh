#!/usr/bin/env bash
# § iPhone SVG blocks (wheel-eg4e.19.17): an agent reply holding a small ```svg (a circle and a text
# label) and an ```svg that does not sanitize (it declares a DOCTYPE). iOS has no SVG decoder, so the
# phone asks wheeld's `svg_image` op to draw the sanitized SVG as PNG and shows that: it logs
# `svg in <agent>: drawn 160x80` for the first and `svg in <agent>: shown as source, it did not
# sanitize` for the second. `simctl io screenshot` saves the chat (WHEEL_SHOTS=<dir>, `ios-svg.png`).
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

REPLY='A picture:

```svg
<svg xmlns="http://www.w3.org/2000/svg" width="160" height="80" viewBox="0 0 160 80">
  <circle cx="40" cy="40" r="30" fill="#2f9e6e"/>
  <text x="80" y="46" font-size="18" fill="#555555">Ready</text>
</svg>
```

And one that does not sanitize:

```svg
<!DOCTYPE svg [<!ENTITY x "boom">]>
<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><text>&x;</text></svg>
```
'
check "the agent echoes the reply with both SVG blocks" w agent send "$P1" "$REPLY" --timeout 30

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
xcrun simctl install "$UDID" "$WHEEL_IOS_APP"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
PLOG="$DATA/Documents/phone.log"
rm -f "$PLOG"
plog() { grep -qF -- "$1" "$PLOG" 2>/dev/null; }

code="$(w remote pair --address 127.0.0.1)"
env SIMCTL_CHILD_WHEEL_PAIR_JSON="$code" SIMCTL_CHILD_WHEEL_START_SPACE=work SIMCTL_CHILD_WHEEL_START_PAGE=1 \
  xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
check "the phone pairs" poll 40 plog "connected to wss://127.0.0.1:$RPORT/ws"
check "case 1: the small SVG draws as a picture" poll 30 plog "svg in $AGENT: drawn 160x80"
# The screen is 402 points wide: a picture shown wider than the block runs off its edge.
check "case 1: the picture shows at its natural size ($(grep -F "svg in $AGENT shown " "$PLOG" | tail -1 | sed 's/.* shown //'))" poll 10 plog "svg in $AGENT shown 160x80"
check "case 2: the SVG that does not sanitize shows its source" poll 30 plog "svg in $AGENT: shown as source, it did not sanitize"
has_log "svg image" "wheeld drew it with svg_image"
sleep 3
xcrun simctl io "$UDID" screenshot "$SHOTS/ios-svg.png" >/dev/null 2>&1
grep -F "svg in" "$PLOG" | sort -u | head -8

[ "$FAIL" -eq 0 ] || { echo "--- phone.log"; grep -E "svg|code block|connected|start page" "$PLOG" | tail -30; }
echo "shots: $SHOTS"
finish
