#!/usr/bin/env bash
# § Turn line (wheel-h3z5.20): a turn that has not ended shows one live line, a spinner, what the
# turn is doing and a timer from the person's message that ticks every second, at every moment
# until the turn ends; the turn's end turns the last work line into "Worked for <turn length>", or
# "Stopped after <length>" when the person stopped it. The fake agent's "work gaps" turn leaves
# stretches where nothing arrives. Drives the real Mac app through its debug server against this
# script's own daemon; /state lists the line as `live <id> · phase … · label "…" · elapsed <s>`.
# Window shots (cacheDisplay) of cases 1, 4 and 7, light and dark, go to $WHEEL_SHOTS (default
# $SCR/shots).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"

export CLAUDE_CONFIG_DIR="$SCR/claude"
use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset

mkdir -p "$SCR/a"
A="$(w space new "$SCR/a")"
P="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
w pane start "$P" agent >/dev/null

launch_app
check "the app connects" wait_st '.connection == "connected"' 20
app_post "/focus?space=$A" >/dev/null

items() { app_get /state | jq -r --arg p "$P" '.panes[$p].transcript_items[]?'; }
live() { items | grep '^live ' | head -1; }
live_has() { live | grep -F -- "$1" >/dev/null; }
live_elapsed() { live | sed 's/.* elapsed //'; }
# wait_phase <phase> [seconds]: wait until the live line is in <phase>.
wait_phase() { local i; for i in $(seq 1 $((${2:-30} * 5))); do live_has "phase $1 ·" && return 0; sleep 0.2; done; return 1; }
# grows: the live line's elapsed seconds grow between two reads 2 s apart.
grows() {
  local a b; a="$(live_elapsed)"; sleep 2; b="$(live_elapsed)"
  echo "    elapsed $a then $b"
  [[ "$a" =~ ^[0-9]+$ && "$b" =~ ^[0-9]+$ ]] && [ "$b" -ge $((a + 1)) ]
}
# last_work: the turn's last work line.
last_work() { items | grep '^work ' | tail -1; }
work_says() { last_work | grep -E -- "$1" >/dev/null; }
shot() { app_post "/window/snapshot?path=$SHOTS/turn-line-$1-$LOOK.png" >/dev/null && echo "     shot $SHOTS/turn-line-$1-$LOOK.png"; }
# case <n> <phase> <label fragment> <seconds to wait>: the live line is in the phase, says the words and its timer grows.
case_live() {
  check "$1 the line's phase is $2" wait_phase "$2" "$4"
  echo "     $(live)"
  check "$1 its label says $3" live_has "label \"$3"
  check "$1 its timer grows between two reads 2 s apart" grows
}

for LOOK in light dark; do
  WHEEL_APPEARANCE=$LOOK
  quit_app
  launch_app
  check "the app connects ($LOOK)" wait_st '.connection == "connected"' 20
  app_post "/focus?space=$A" >/dev/null
  echo "=== $LOOK"
  w agent send --timeout 0 "$P" "hold first 3000 work gaps" >/dev/null
  case_live "6 (nothing has arrived yet)" waiting-model "Waiting for the model" 5
  case_live "2 (a thought streams)" thinking "Thinking" 15
  case_live "1 (between two calls)" waiting-model "Waiting for the model" 20
  shot 1
  case_live "4 (a tool runs)" tool "Working · Run the parser's tests" 20
  shot 4
  case_live "3 (reply text streams)" writing "Writing the reply" 30
  check "7 the turn ends and the live line goes" wait_st ".panes[\"$P\"].transcript_items | map(select(startswith(\"live \"))) | length == 0" 30
  echo "     $(last_work)"
  check "7 the last work line says Worked for the turn's length" work_says 'label "Worked for ([0-9]+m )?[0-9]+s"'
  shot 7
done

# 5: while a permission request waits
echo "=== permission"
w agent send --timeout 0 "$P" "permission" >/dev/null
check "5 a request waits: the line's phase is permission" wait_phase permission 10
echo "     $(live)"
check "5 its label says it waits for the permission" live_has 'label "Waiting for your permission'
check "5 its timer grows between two reads 2 s apart" grows
w agent cancel "$P" >/dev/null
wait_st ".panes[\"$P\"].transcript_items | map(select(startswith(\"live \"))) | length == 0" 15

# 8: a turn stopped
echo "=== stopped"
w agent send --timeout 0 "$P" "hold first 3000 work gaps" >/dev/null
check "8 the second turn runs a tool" wait_phase tool 25
w agent cancel "$P" >/dev/null
check "8 stopped: the live line goes" wait_st ".panes[\"$P\"].transcript_items | map(select(startswith(\"live \"))) | length == 0" 15
echo "     $(last_work)"
check "8 the last work line says Stopped after the turn's length" work_says 'label "Stopped after ([0-9]+m )?[0-9]+s"'
finish
