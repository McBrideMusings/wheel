#!/usr/bin/env bash
# Per-reply playback (wheel-e1xk.14). The last reply of a finished turn draws a play button just to
# the right of its copy button. A press on it speaks that reply (the app keeps the text it handed
# the synthesizer and the key of the reply being read), the button then offers Stop, a second press
# stops it, and none of this touches the conversation's read-aloud toggle. The app runs hidden and
# is never activated or sent real input: the press is the debug route `POST /reply/play`, and the
# window shots draw the view hierarchy. The synthesizer's audio is not captured: the app's own
# count, text and playing key stand for it.
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

say() { curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" --data "$1" "http://127.0.0.1:$DEBUG_PORT/agent?agent=$S" >/dev/null; }
spoken() { st --arg p "$P" '.panes[$p].read_aloud.spoken'; }
playing() { st --arg p "$P" '.panes[$p].read_aloud.playing'; }

say "a reply to play, long enough to still be spoken when the second press lands: alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november oscar papa quebec romeo sierra tango"
wait_st --arg p "$P" '(.panes[$p].transcript | length) > 1' 15
sleep 1
ROW="$(st -r --arg p "$P" '.panes[$p].transcript_items[]' | awk '$1=="agent"{print $2; exit}')"

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
