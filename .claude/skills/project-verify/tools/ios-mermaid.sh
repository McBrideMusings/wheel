#!/usr/bin/env bash
# § iPhone Mermaid diagrams (wheel-eg4e.19.16): an agent reply holding a ```mermaid flowchart and a
# block that does not parse, echoed in an agent pane and in an orchestrator pane of the same space.
# The phone asks wheeld's `diagram` op to lay each out and draw it as PNG, and shows that: it logs
# `mermaid in <agent>: drawn <w>x<h>` for the flowchart and `mermaid in <agent> could not draw: <error>`
# for the other, in both panes with the same size. `simctl io screenshot` saves each pane's chat
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
agent_of() { w space ls | jq -r --arg p "$1" '[.spaces[].root | .. | select(.id? == $p)][0].agent'; }
AGENT="$(agent_of "$P1")"
check "wheeld started the agent pane" test -n "$AGENT" -a "$AGENT" != null
ORCH="$(w orchestrator start --space "$WORK" --pull-after 3600)"
OAGENT=""
for _ in $(seq 50); do OAGENT="$(agent_of "$ORCH")"; [ -n "$OAGENT" ] && [ "$OAGENT" != null ] && break; sleep 0.2; done
check "wheeld started the orchestrator pane $ORCH" test -n "$OAGENT" -a "$OAGENT" != null

REPLY='The plan:

```mermaid
flowchart LR
  A[Plan] --> B{Green?}
  B -->|yes| C[Land]
  B -->|no| A
```

And a broken one:

```mermaid
flowchart LR
  A -->
  ((((
```
'
check "the agent echoes the reply with both Mermaid blocks" w agent send "$P1" "$REPLY" --timeout 30
check "the orchestrator echoes the same reply" w agent send "$ORCH" "$REPLY" --timeout 30
PAGES="$(w space ls | jq -r --arg s "$WORK" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane") | .id] | join(" ")')"
OPAGE=0; n=0; for p in $PAGES; do n=$((n + 1)); [ "$p" = "$ORCH" ] && OPAGE=$n; done
check "the orchestrator pane is a page of the work space ($PAGES)" test "$OPAGE" -gt 0

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
xcrun simctl install "$UDID" "$WHEEL_IOS_APP"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
PLOG="$DATA/Documents/phone.log"
rm -f "$PLOG"
plog() { grep -qF -- "$1" "$PLOG" 2>/dev/null; }
size_in() { grep -F "mermaid in $1: drawn " "$PLOG" | tail -1 | sed 's/.*drawn //'; }

code="$(w remote pair --address 127.0.0.1)"
# The pairing code is single use: a relaunched app reconnects with the credential it saved.
launch() {
  xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1
  env "${@:2}" SIMCTL_CHILD_WHEEL_START_SPACE=work SIMCTL_CHILD_WHEEL_START_PAGE="$1" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
}
launch 1 SIMCTL_CHILD_WHEEL_PAIR_JSON="$code"
check "the phone pairs" poll 40 plog "connected to wss://127.0.0.1:$RPORT/ws"
check "case 1: the agent pane draws the flowchart" poll 30 plog "mermaid in $AGENT: drawn "
check "case 3: the broken block stays code with its error" poll 30 plog "mermaid in $AGENT could not draw: "
# The screen is 402 points wide: a diagram shown wider than the block runs off its edge.
fits() { local w; w="$(grep -F "mermaid in $1 shown " "$PLOG" | tail -1 | sed 's/.* shown \([0-9]*\)x.*/\1/')"; [ -n "$w" ] && [ "$w" -gt 100 ] && [ "$w" -le 378 ]; }
check "case 1: the diagram fits the block's width ($(grep -F "mermaid in $AGENT shown " "$PLOG" | tail -1 | sed 's/.* shown //'))" poll 10 fits "$AGENT"
sleep 3
xcrun simctl io "$UDID" screenshot "$SHOTS/ios-mermaid.png" >/dev/null 2>&1

launch "$OPAGE"
check "case 2: the orchestrator pane draws the flowchart" poll 40 plog "mermaid in $OAGENT: drawn "
check "case 2: the orchestrator's broken block stays code with its error" poll 30 plog "mermaid in $OAGENT could not draw: "
A_SIZE="$(size_in "$AGENT")"; O_SIZE="$(size_in "$OAGENT")"
check "case 2: both panes draw the same size ($A_SIZE, $O_SIZE)" test -n "$A_SIZE" -a "$A_SIZE" = "$O_SIZE"
sleep 3
xcrun simctl io "$UDID" screenshot "$SHOTS/ios-mermaid-orchestrator.png" >/dev/null 2>&1
grep -F "mermaid in" "$PLOG" | sort -u | head -8

[ "$FAIL" -eq 0 ] || { echo "--- phone.log"; grep -E "mermaid|code block|connected|start page" "$PLOG" | tail -30; }
echo "shots: $SHOTS"
finish
