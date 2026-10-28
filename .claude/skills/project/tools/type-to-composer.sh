#!/usr/bin/env bash
# Typing in a focused agent pane goes to its composer (wheel-e916.8). A printable key typed while the
# transcript holds the keyboard joins the composer's draft and moves the keyboard there; keys with
# ⌘ or ⌥ keep their own meaning. Keys go in as events through the app's event path.
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
type_key() { curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" "http://127.0.0.1:$DEBUG_PORT/agent/type?session=$S&text=$1&mods=${2:-}" >/dev/null; }

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
finish
