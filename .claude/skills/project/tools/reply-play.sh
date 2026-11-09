#!/usr/bin/env bash
# Per-reply playback (wheel-e1xk.14). The last reply of a finished turn draws a play button just to
# the right of its copy button. A press on it speaks that reply (the app keeps the text it
# handed the synthesizer and the key of the reply being read), the button then offers Stop, a second
# click stops it, and none of this touches the conversation's read-aloud toggle. The synthesizer's
# audio is not captured: the app's own count, text and playing key stand for it.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
unset WHEEL_SPACE WHEEL_PANE
mkdir -p "$SCR/alpha"
A="$(w space new "$SCR/alpha")"
launch_app
wait_st --arg s "$A" '.active == $s' 20
P="$(st --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
app_post "/key?keys=1" >/dev/null
wait_st --arg p "$P" '.panes[$p].kind == "agent"' 15
S="$(st --arg p "$P" '.panes[$p].agent')"
app_post "/window/front" >/dev/null

activate() {
  app_post "/app/activate" >/dev/null
  local active; active() { app_get /input/cursor | jq -r .app_active; }
  [ "$(active)" = true ] && return 0
  local try
  for try in 1 2 3; do
    osascript -e "tell application \"System Events\" to set frontmost of (first process whose unix id is $(app_pid)) to true" >/dev/null 2>&1
    sleep 0.5
    [ "$(active)" = true ] && return 0
  done
  return 1
}
ACTIVE=no; activate && ACTIVE=yes
WIN="$(st '.windows[0].id')"
click() {
  local out try
  for try in 1 2 3; do
    activate >/dev/null
    out="$(curl -s -X POST -H "Authorization: Bearer $(TOKEN)" "http://127.0.0.1:$DEBUG_PORT/input/mouse?window=$WIN&action=click&x=$CX&y=$CY")"
    case "$out" in *'"error"'*) sleep 0.5;; *) return 0;; esac
  done
  echo "     click refused: $out"
}
say() { curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" --data "$1" "http://127.0.0.1:$DEBUG_PORT/agent?agent=$S" >/dev/null; }
spoken() { st --arg p "$P" '.panes[$p].read_aloud.spoken'; }
playing() { st --arg p "$P" '.panes[$p].read_aloud.playing'; }
control() { app_get /chat/controls | jq -c --arg l "$1" '[.[] | select(.label == $l)][0]'; }
aim() { CX="$(echo "$1" | jq '.frame.x + .frame.w / 2')"; CY="$(echo "$1" | jq '.frame.y + .frame.h / 2')"; }

say "a reply to play, long enough to still be spoken when the second press lands: alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november oscar papa quebec romeo sierra tango"
wait_st --arg p "$P" '(.panes[$p].transcript | length) > 1' 15
sleep 1
ROW="$(st -r --arg p "$P" '.panes[$p].transcript_items[]' | awk '$1=="agent"{print $2; exit}')"
echo "NOT DRIVEN: real pointer click (the chat controls registry does not list the reply buttons); pressing by /reply/play. The window shots show the two buttons."
eq "nothing is spoken before a press" "$(spoken)" "0"
eq "the read-aloud toggle is off" "$(st --arg p "$P" '.panes[$p].reads_aloud')" "false"
app_post "/window/snapshot?path=$SHOTS/reply-play-idle.png" >/dev/null

app_post "/reply/play?agent=$S&row=$ROW" >/dev/null
wait_st --arg p "$P" '.panes[$p].read_aloud.spoken >= 1' 10
eq "a press on play speaks the reply once" "$(spoken)" "1"
eq "what was spoken is the reply's prose" "$(st --arg p "$P" '.panes[$p].read_aloud.last | contains("alpha bravo charlie")')" "true"
eq "the app says which reply is being read" "$(playing)" "$S#$ROW"
eq "the read-aloud toggle did not change" "$(st --arg p "$P" '.panes[$p].reads_aloud')" "false"
app_post "/window/snapshot?path=$SHOTS/reply-play-playing.png" >/dev/null

app_post "/reply/play?agent=$S&row=$ROW" >/dev/null
wait_st --arg p "$P" '.panes[$p].read_aloud.playing == null' 5
eq "a second press stops it" "$(playing)" "null"
eq "stopping did not speak again" "$(spoken)" "1"
app_post "/window/snapshot?path=$SHOTS/reply-play-stopped.png" >/dev/null
echo "shots: $SHOTS"
