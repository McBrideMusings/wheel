#!/usr/bin/env bash
# The question card (wheel-caif): an agent's AskUserQuestion shows as a question with one button per
# option and Skip, not as "Use a tool". Drives the real Mac app against this script's own daemon
# with the fake agent ("ask a question" calls AskUserQuestion with two questions).
# Cases: 1 the request is the first question, its header the reason, options Red, Blue, Skip;
# 2 Return picks nothing (no option is the default); 3 a pick asks the second question, whose pick
# answers the tool call with both answers; 4 Esc skips and the tool call is refused;
# 5 the card's drawing (WHEEL_SHOTS=<dir> keeps question-card-<look>.png; run once per
# WHEEL_APPEARANCE=light|dark).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
LOOK="${WHEEL_APPEARANCE:-light}"
mkdir -p "$SHOTS"

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
mkdir -p "$SCR/a"
A="$(w space new "$SCR/a")"
launch_app
wait_st --arg s "$A" '.active == $s' 20
P="$(st --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
w pane start "$P" agent >/dev/null
wait_st --arg p "$P" '.panes[$p].kind == "agent"' 15
S="$(st --arg p "$P" '.panes[$p].agent')"

perm() { st --arg s "$S" '[.perms[] | select(.agent == $s)][0]'; }
waiting() { wait_st --arg s "$S" '[.perms[] | select(.agent == $s)] | length == 1' 15; }
gone() { wait_st --arg s "$S" '[.perms[] | select(.agent == $s)] | length == 0' 15; }
send() { curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" --data "$1" "http://127.0.0.1:$DEBUG_PORT/agent?agent=$S" >/dev/null; }
focus_composer() { app_post "/agent/focus?agent=$S" >/dev/null; sleep 0.3; }
said() { st --arg p "$P" '[.panes[$p].transcript[] | tostring] | join("\n")' | grep -qE -- "$1"; }
names() { perm | jq -r '[.options[].name] | join(",")'; }

send "ask a question"
waiting
eq "1: the first question is the title" "$(perm | jq -r .title)" "Which color?"
eq "1: its header is the reason" "$(perm | jq -r .reason)" "Color"
eq "1: one button per option, then Skip" "$(names)" "Red,Blue,Skip"
sleep 2
app_post "/window/snapshot?path=$SHOTS/question-card-$LOOK.png" >/dev/null
echo "     shot $SHOTS/question-card-$LOOK.png"

focus_composer
app_post "/key?keys=enter" >/dev/null
sleep 1
eq "2: Return picked nothing" "$(perm | jq -r .title)" "Which color?"

w perm answer "$(perm | jq -r .id)" pick:1 >/dev/null
wait_st --arg s "$S" '[.perms[] | select(.agent == $s)][0].title == "Which size?"' 15
eq "3: a pick asks the second question" "$(names)" "Small,Large,Skip"
w perm answer "$(perm | jq -r .id)" pick:0 >/dev/null
gone
check "3: the agent got both answers" poll 10 said 'answers=\{"Which color\?":"Blue","Which size\?":"Small"\}'

send "ask a question"
waiting
focus_composer
app_post "/key?keys=escape" >/dev/null
gone
check "4: Esc skipped and the tool call was refused" poll 10 said 'question: deny'

finish
