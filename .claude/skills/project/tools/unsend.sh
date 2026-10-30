#!/usr/bin/env bash
# Unsend a message before the agent answers, and edit or remove queued messages (wheel-h3z5.30).
# Esc or Up in an empty composer takes back a message the agent has produced nothing for (its text
# returns to the composer, the transcript forgets it, no turn ran); after the first event the same
# keys leave it alone, and Esc stops the turn. Each queued message can be edited (out of the queue,
# its text in the composer) or removed, and Clear queue empties the queue. Every attached client
# sees each change. Keys go in as events through the app's event path with the composer focused.
# Window shots (cacheDisplay) go to $WHEEL_SHOTS (default $SCR/shots).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"
use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
mkdir -p "$SCR/alpha"
A="$(w space new "$SCR/alpha")"
WHEEL_APPEARANCE=light launch_app
wait_st --arg s "$A" '.active == $s' 20
P="$(st --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
app_post "/key?keys=1" >/dev/null
wait_st --arg p "$P" '.panes[$p].kind == "agent"' 15
S="$(st --arg p "$P" '.panes[$p].session')"
w agent wait "$S" --timeout 30 >/dev/null

pane() { st --arg p "$P" "$@"; }
composer() { pane '.panes[$p].composer'; }
queued_texts() { pane '[.panes[$p].queued[].text] | join(",")'; }
queued_turn() { pane --argjson i "$1" '.panes[$p].queued[$i].turn'; }
send() { curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" --data "$1" "http://127.0.0.1:$DEBUG_PORT/agent?session=$S" >/dev/null; }
focus() { app_post "/agent/focus?session=$S" >/dev/null; sleep 0.3; }
queue_click() { app_post "/agent/queue?session=$S&$1" >/dev/null; }
events() { w agent read "$S" --json | jq -r "$@"; }
users() { events '[.events[] | select(.kind == "user") | .text] | join(",")'; }
ready() { w agent wait "$S" --timeout 30 >/dev/null; }
# A turn's first event: the fake agent's thought, 0.5 s after the turn starts.
first_event() { wait_st --arg p "$P" '[.panes[$p].transcript[] | select(startswith("thought"))] | length > 0' 15; }
reset_chat() {
  app_post "/agent/draft?session=$S" >/dev/null
  w agent cancel "$S" >/dev/null 2>&1
  ready
}

# 1. Send, then Esc before any event: the message is gone, its text is back, no turn ran.
focus
send "hold first 20000 work slowly 30"
app_post "/key?keys=escape" >/dev/null
wait_st --arg p "$P" '.panes[$p].composer == "hold first 20000 work slowly 30"' 5
eq "1: Esc before any event puts the text back in the composer" "$(composer)" "hold first 20000 work slowly 30"
eq "1: the transcript no longer holds the message" "$(pane '.panes[$p].transcript | length')" "0"
ready
eq "1: the daemon's transcript is empty, no turn ran" "$(events '.events | length')" "0"
has_log "running message unsent before any event" "1: the daemon logged the message unsent before any event"
app_post "/agent/draft?session=$S" >/dev/null

# 2. Send, then Up in the empty composer: the same.
focus
send "hold first 20000 work slowly 30"
app_post "/key?keys=arrow_up" >/dev/null
wait_st --arg p "$P" '.panes[$p].composer == "hold first 20000 work slowly 30"' 5
eq "2: Up before any event puts the text back in the composer" "$(composer)" "hold first 20000 work slowly 30"
eq "2: the transcript no longer holds the message" "$(pane '.panes[$p].transcript | length')" "0"
ready
eq "2: the daemon's transcript is empty, no turn ran" "$(events '.events | length')" "0"
app_post "/agent/draft?session=$S" >/dev/null

# 3. Send, wait for the first event, Esc: the turn stops and the message stays.
send "work slowly 30"
first_event
eq "3: the message is answered, so it is not unsendable" "$(pane '.panes[$p].unanswered_turn')" "null"
focus
app_post "/key?keys=escape" >/dev/null
ready
eq "3: the message stays in the transcript" "$(pane '[.panes[$p].transcript[] | select(startswith("user"))] | length')" "1"
eq "3: the composer stays empty" "$(composer)" ""
eq "3: the turn ended as cancelled" "$(events '[.events[] | select(.kind == "turn_end") | .stop_reason] | last')" "cancelled"
app_post "/agent/draft?session=$S" >/dev/null

# 4. Queue two behind a running turn, Remove the first: one left.
send "work slowly 600"
first_event
send "m1"
send "m2"
wait_st --arg p "$P" '(.panes[$p].queued | length) == 2' 5
eq "4: two messages queued, in order" "$(queued_texts)" "m1,m2"
queue_click "action=remove&turn=$(queued_turn 0)"
wait_st --arg p "$P" '(.panes[$p].queued | length) == 1' 5
eq "4: Remove leaves the second" "$(queued_texts)" "m2"
eq "4: the daemon dropped m1 too" "$(users)" "work slowly 30,work slowly 600,m2"

# 5. Edit the queued one: it leaves the queue and its text is in the composer.
queue_click "action=edit&turn=$(queued_turn 0)"
wait_st --arg p "$P" '(.panes[$p].queued | length) == 0' 5
wait_st --arg p "$P" '.panes[$p].composer == "m2"' 5
eq "5: Edit empties the queue" "$(queued_texts)" ""
eq "5: its text is in the composer" "$(composer)" "m2"
eq "5: the daemon dropped m2 too" "$(users)" "work slowly 30,work slowly 600"
app_post "/agent/draft?session=$S" >/dev/null

# 6. Queue three, Clear queue: none left.
send "c1"
send "c2"
send "c3"
wait_st --arg p "$P" '(.panes[$p].queued | length) == 3' 5
eq "6: three messages queued, in order" "$(queued_texts)" "c1,c2,c3"
shot() { app_post "/window/snapshot?path=$SHOTS/unsend-$1.png" >/dev/null && echo "shot $SHOTS/unsend-$1.png"; }
shot light
queue_click "action=clear"
wait_st --arg p "$P" '(.panes[$p].queued | length) == 0' 5
eq "6: Clear queue leaves none" "$(queued_texts)" ""
eq "6: the daemon dropped all three" "$(users)" "work slowly 30,work slowly 600"
eq "6: the running turn is untouched" "$(pane '.panes[$p].state | startswith("Working")')" "true"

# 7. A second client acts through the CLI; the app, attached, sees each change at once.
send "d1"
send "d2"
send "d3"
wait_st --arg p "$P" '(.panes[$p].queued | length) == 3' 5
T="$(queued_turn 1)"
w agent unsend "$S" "$T"
wait_st --arg p "$P" '(.panes[$p].queued | length) == 2' 5
eq "7: a CLI unsend of d2 shows in the app" "$(queued_texts)" "d1,d3"
w agent clear-queue "$S"
wait_st --arg p "$P" '(.panes[$p].queued | length) == 0' 5
eq "7: a CLI clear shows in the app" "$(queued_texts)" ""
eq "7: the CLI refuses an unsend of the answered message" "$(w agent unsend "$S" 1 2>&1 | grep -c 'no unanswered message')" "1"
has_log "queue cleared" "7: the daemon logged the clear"
w agent clear-queue "$S" >/dev/null 2>&1
eq "7: clearing an empty queue exits 6" "$?" "6"
reset_chat

# Window shots of a queued message's actions, light and dark.
send "work slowly 600"
first_event
send "first queued message"
send "second queued message"
wait_st --arg p "$P" '(.panes[$p].queued | length) == 2' 5
sleep 0.5
shot light-queued
kill_app
WHEEL_APPEARANCE=dark launch_app
wait_st --arg p "$P" '(.panes[$p].queued | length) == 2' 20
sleep 1
shot dark-queued

finish
