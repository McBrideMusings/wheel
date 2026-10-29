#!/usr/bin/env bash
# Typing goes to the focused chat's composer (wheel-e916.8, wheel-h3z5.19). A printable key typed
# anywhere in the window while the focused pane is an agent pane and no editable text input holds the
# keyboard joins the composer's draft and moves the keyboard there: with the transcript, the window
# itself (a pane header, a header button or the title bar was clicked), the sidebar, a read-only text
# or an empty new chat holding it. Keys with ⌘ or ⌥ keep their own meaning, and an editable field
# (a text view, the command palette, Settings) or a terminal pane keeps its keys. Keys go in as
# events through the app's event path.
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
S="$(st --arg p "$P" '.panes[$p].session')"

composer() { st --arg p "$P" '.panes[$p].composer'; }
# type_key <text> [mods] [from] [to]: a key event; from names what holds the keyboard first.
type_key() { curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" "http://127.0.0.1:$DEBUG_PORT/agent/type?session=${SESSION_OF:-$S}&text=$1&mods=${2:-}&from=${3:-transcript}&to=${4:-}" >/dev/null; }
clear_draft() { app_post "/agent/draft?session=$S" >/dev/null; wait_st --arg p "$P" '.panes[$p].composer == ""' 5; }

# A conversation with a row to click.
curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" --data 'say something' "http://127.0.0.1:$DEBUG_PORT/agent?session=$S" >/dev/null
wait_st --arg p "$P" '(.panes[$p].transcript | length) > 1' 15

# 'h' with the transcript's scroll view holding the keyboard.
type_key h
wait_st --arg p "$P" '.panes[$p].composer == "h"' 5
eq "h typed over the transcript lands in the draft" "$(composer)" "h"
wait_st '.first_responder | test("TextView")' 5
eq "the composer holds the keyboard" "$(st '.first_responder | test("TextView")')" "true"
app_post "/key?keys=i" >/dev/null
wait_st --arg p "$P" '.panes[$p].composer == "hi"' 5
eq "the next key goes to the composer natively, after the first" "$(composer)" "hi"

# ⌘C and ⌥X are not typing.
app_post "/agent/draft?session=$S" >/dev/null
type_key c cmd
type_key x opt
sleep 0.5
eq "⌘C and ⌥X add nothing to the draft" "$(composer)" ""

# Anywhere in the window: the pane header, a header button and the title bar leave nothing holding
# the keyboard (window); the sidebar's list and a read-only reply are not text inputs.
for from in window sidebar readonly; do
  clear_draft
  type_key hello "" "$from"
  wait_st --arg p "$P" '.panes[$p].composer == "hello"' 5
  eq "hello typed with the keyboard on '$from' lands in the draft" "$(composer)" "hello"
done

# An editable text field keeps its keys.
clear_draft
type_key hello "" editable
sleep 0.5
eq "an editable text view keeps its keys" "$(composer)" ""

# The command palette's field keeps its keys.
app_post "/palette?action=open" >/dev/null
type_key hello "" window
sleep 0.5
eq "with the command palette open the draft stays empty" "$(composer)" ""
app_post "/palette?action=close" >/dev/null

# The Settings window's keys stay there.
app_post "/settings?action=open" >/dev/null
sleep 1
type_key hello "" window settings
sleep 0.5
eq "a key typed in Settings adds nothing to the draft" "$(composer)" ""
app_post "/settings?action=close" >/dev/null

# An empty new chat: no rows, so no transcript scroll view for the key to be found in.
app_post "/split?pane=$P&dir=right" >/dev/null
wait_st --arg s "$A" --arg p "$P" '.spaces[] | select(.id==$s) | .focus != $p' 10
P2="$(st --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
app_post "/key?keys=1" >/dev/null
wait_st --arg p "$P2" '.panes[$p].kind == "agent"' 15
S2="$(st --arg p "$P2" '.panes[$p].session')"
SESSION_OF="$S2" type_key hello "" window
wait_st --arg p "$P2" '.panes[$p].composer == "hello"' 5
eq "hello typed in an empty new chat lands in its draft" "$(st --arg p "$P2" '.panes[$p].composer')" "hello"

# A focused terminal pane keeps its keys: the first chat's draft stays empty.
app_post "/split?pane=$P2&dir=down" >/dev/null
wait_st --arg s "$A" --arg p "$P2" '.spaces[] | select(.id==$s) | .focus != $p' 10
app_post "/key?keys=t" >/dev/null
P3="$(st --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
wait_st --arg p "$P3" '.panes[$p].kind == "terminal"' 15
clear_draft
type_key x "" window
sleep 0.5
eq "with a terminal pane focused no chat's draft changes" "$(composer)$(st --arg p "$P2" '.panes[$p].composer')" "hello"
finish
