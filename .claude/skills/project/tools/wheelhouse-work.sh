#!/usr/bin/env bash
# § Wheelhouse Implement and Swarm views (wheel-cmbu.6): two artifacts served by the one
# `wheelhouse serve` process from the agents the core follows. A pane's declared work item and
# stage show on the Implement page and its state on the Swarm page; typing a brief and pressing
# Start runs an agent through the core's spawn action; another space's agents never show. Drives
# the real `wheel` against this script's own daemon with a fake agent. Needs `cargo build -p
# wheelhouse -p wheeld -p wheel-cli --examples --bins`, jq. No app.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HOUSE="$BIN/wheelhouse"
[ -x "$HOUSE" ] || { echo "missing $HOUSE: run cargo build -p wheelhouse" >&2; exit 2; }
command -v jq >/dev/null || { echo "jq is not installed" >&2; exit 2; }

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
export WHEEL_URL="ws://127.0.0.1:$PORT/ws" WHEEL_TOKEN_FILE="$INST/token"
export PATH="$(dirname "$WHEEL"):$PATH"

mkdir -p "$SCR/proj" "$SCR/other"
SP="$(w space new "$SCR/proj")"
OTHER="$(w space new "$SCR/other")"
PANE="$(w space ls | jq -r --arg s "$SP" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
OPANE="$(w space ls | jq -r --arg s "$OTHER" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"

for v in implement swarm; do
  printf '[artifacts.%s-native]\nmcp = "exec %s serve"\nshared = true\ntitle = "%s"\n' "$v" "$HOUSE" "$v" >>"$INST/wheel.toml"
done
# A call's answer: the page's components as JSON, or the tool's own error text. The asking pane is
# the space's first pane, which `spawn_agent` splits.
call() {
  local out; out="$(w artifact call "$1-native" "$2" ${4:+"$4"} --scope machine --space "$3" --pane "$PANE" 2>&1)"
  printf %s "$out" | jq -r '.resource.text' 2>/dev/null || printf %s "$out"
}
has() { printf %s "$1" | grep -q -- "$2"; }
hasnt() { ! has "$1" "$2"; }

# An agent in each space, the first with a declared item and stage.
A1="$(cd "$SCR/proj" && "$HOUSE" agents spawn "hello" --pane "$PANE" --model haiku 2>&1)"
A2="$(cd "$SCR/other" && "$HOUSE" agents spawn "hello" --pane "$OPANE" --model haiku 2>&1)"
WHEEL_PANE="$A1" w work set --item bead:wheel-x1 --stage verify --note "running the checks" >/dev/null
WHEEL_PANE="$A2" w work set --item bead:other-y2 --stage edit >/dev/null

# A page follows agents while a pane shows it: a web pane of each artifact in this space, and a
# read of the resource subscribes the host to it.
for v in implement swarm; do
  P="$(w pane split "$PANE" right | awk '{print $1}')"
  w pane kind "$P" web --artifact "$v-native" --scope machine >/dev/null
  w artifact read "$v-native" "a2ui://$v-native" --scope machine --space "$SP" >/dev/null 2>&1
done
shows_item() { has "$(call implement get_page "$SP")" "bead wheel-x1"; }
check "the Implement page shows the declared item within 10 seconds" poll 10 shows_item
IMP="$(call implement get_page "$SP")"
check "it shows the item's stage" has "$IMP" '"title":"Verify"'
check "it shows the note" has "$IMP" "running the checks"
check "it holds nothing of the other space's item" hasnt "$IMP" "other-y2"

SW="$(call swarm get_page "$SP")"
check "the Swarm page lists the worker's pane" has "$SW" "\"text\":\"$A1\""
check "the Swarm page holds nothing of the other space's worker" hasnt "$SW" "\"text\":\"$A2\""

# Start needs a brief; typing one and pressing Start runs an agent in a new pane of this space.
check "Start is disabled with no brief" has "$(call swarm get_page "$SP")" '"disabled":true'
call swarm set_brief "$SP" '{"value":"permission please"}' >/dev/null
OUT="$(call swarm start_agent "$SP")"
check "Start reports the new pane" has "$OUT" "Started the agent in pane"
eq "the space now has two agents" "$(cd "$SCR/proj" && "$HOUSE" agents ls | jq length)" 2
check "the typed brief is cleared after Start" has "$OUT" '"value":""'

# The agent tool an agent calls directly.
OUT="$(call swarm spawn_agent "$SP" '{"brief":"hi"}')"
check "spawn_agent answers with a pane id" test -n "$OUT" -a "$OUT" != "$A1"
finish
