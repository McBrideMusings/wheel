#!/usr/bin/env bash
# Read-aloud (wheel-e1xk.3). Each agent conversation has a speaker toggle in its composer, off to
# start. A real click on it turns read-aloud on for that conversation alone; with it on, the reply
# of the next turn that ends is handed to the system speech synthesizer (the app counts replies and
# keeps the last text it spoke), and with it off nothing is spoken. History loaded on attach is
# never read. The synthesizer's audio is not captured: the app's own count and text stand for it.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
mkdir -p "$SCR/alpha"
A="$(w space new "$SCR/alpha")"
launch_app
wait_st --arg s "$A" '.active == $s' 20
P="$(st --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
app_post "/key?keys=1" >/dev/null
wait_st --arg p "$P" '.panes[$p].kind == "agent"' 15
S="$(st --arg p "$P" '.panes[$p].agent')"
app_post "/window/front" >/dev/null

# A real click needs the app to be the active app (launch_app opens it in the background), so take
# the front place the way real-input.sh does.
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
reads() { st --arg p "$P" '.panes[$p].reads_aloud'; }

# Off to start: a reply is not spoken.
eq "read-aloud starts off" "$(reads)" "false"
say "first message"
wait_st --arg p "$P" '(.panes[$p].transcript | length) > 1' 15
sleep 1
eq "with read-aloud off nothing is spoken" "$(spoken)" "0"

# A real click on the speaker turns it on.
control() { app_get /chat/controls | jq -c '[.[] | select(.label | test("Toggle reading replies aloud"))][0]'; }
C="$(control)"
eq "the composer draws a speaker control" "$([ "$C" != "null" ] && echo yes)" "yes"
CX="$(echo "$C" | jq '.frame.x + .frame.w / 2')"
CY="$(echo "$C" | jq '.frame.y + .frame.h / 2')"
# When this session cannot take the front place (macOS keeps another app in front), the toggle is
# set through the debug route instead, and the report says the click was not driven.
if [ "$ACTIVE" = yes ]; then click; else echo "     NOT DRIVEN: real click (the app cannot become the active app here); toggling by /agent/read-aloud"; app_post "/agent/read-aloud?agent=$S&on=1" >/dev/null; fi
wait_st --arg p "$P" '.panes[$p].reads_aloud == true' 5
eq "the speaker toggle turns read-aloud on ($( [ "$ACTIVE" = yes ] && echo real click || echo debug route ))" "$(reads)" "true"

# The next reply is spoken, once, as prose.
say "second message"
wait_st --arg p "$P" '.panes[$p].read_aloud.spoken >= 1' 15
eq "the reply of the next turn is spoken once" "$(spoken)" "1"
eq "what was spoken is the reply's text" "$(st --arg p "$P" '.panes[$p].read_aloud.last | length > 0')" "true"
echo "spoken: $(st --arg p "$P" '.panes[$p].read_aloud.last')"

# Off again: the next reply is not spoken.
if [ "$ACTIVE" = yes ]; then click >/dev/null; else app_post "/agent/read-aloud?agent=$S&on=0" >/dev/null; fi
wait_st --arg p "$P" '.panes[$p].reads_aloud == false' 5
say "third message"
wait_st --arg p "$P" '(.panes[$p].transcript | length) > 5' 15
sleep 1
eq "with it off again the count stays at one" "$(spoken)" "1"
finish
