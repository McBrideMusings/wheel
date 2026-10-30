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
type_key hello "" keep
sleep 0.5
eq "with the command palette open the draft stays empty" "$(composer)" ""
eq "the palette's own field took the word" "$(st '.palette.query')" "hello"
app_post "/palette?action=close" >/dev/null

# The Settings window's keys stay there.
app_post "/settings?action=open" >/dev/null
sleep 1
app_post "/settings?action=tab&tab=chat" >/dev/null
sleep 1
type_key hello "" search settings
sleep 0.5
eq "a key typed in Settings adds nothing to the draft" "$(composer)" ""
eq "Settings' search field took the word" "$(st '.settings.chat.query + .settings.terminal.query')" "hello"
app_post "/settings?action=close" >/dev/null

# The keys that scroll a transcript keep scrolling it: the monitor leaves Page Up, Home, an arrow
# and End alone, so the draft stays empty and the transcript moves.
for _ in 1 2 3 4 5 6; do
  curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" --data 'a reply long enough to take a few lines of the transcript, so it scrolls' "http://127.0.0.1:$DEBUG_PORT/agent?session=$S" >/dev/null
done
wait_st --arg p "$P" '(.panes[$p].transcript | length) > 12' 20
app_post "/window/front" >/dev/null
clear_draft
from_end() { st --arg p "$P" '.panes[$p].from_end'; }
scroll_key() { app_post "/agent/key?session=$S&key=$1" >/dev/null; sleep 0.5; }
scroll_key end
END0="$(from_end)"
scroll_key home
HOME0="$(from_end)"
eq "Home moves the transcript away from its end" "$([ "$HOME0" -gt "$END0" ] && echo yes)" "yes"
scroll_key end
eq "End returns it to the end" "$(from_end)" "$END0"
scroll_key pageUp
UP="$(from_end)"
eq "Page Up scrolls the transcript up" "$([ "$UP" -gt "$END0" ] && echo yes)" "yes"
scroll_key pageDown
eq "Page Down scrolls it back down" "$([ "$(from_end)" -lt "$UP" ] && echo yes)" "yes"
scroll_key up
UP1="$(from_end)"
scroll_key down
eq "an arrow key steps the transcript" "$([ "$UP1" != "$(from_end)" ] && echo yes)" "yes"
eq "the scrolling keys left the draft empty" "$(composer)" ""
scroll_key end

# ⌘C copies a transcript selection and adds nothing to the draft.
ROW="$(st --arg p "$P" '.panes[$p].transcript_items | map(select(startswith("agent "))) | .[-1] | split(" ")[1]')"
# The pasteboard is the machine's, so another check running at the same moment can overwrite it:
# each attempt reads the selected text from the select route's answer, marks the pasteboard, sends
# ⌘C and waits for the marker to go; the case passes when an attempt ends with the selected text.
COPIED=""
for attempt in 1 2 3; do
  SELECTED="$(app_post "/transcript/select?session=$S&from_row=$ROW&from_offset=0&to_row=$ROW&to_offset=100000&mouse=1" | jq -r .pasteboard)"
  printf 'marker-%s-%s' "$$" "$attempt" | pbcopy
  type_key c cmd
  for _ in $(seq 1 20); do
    COPIED="$(pbpaste)"
    [ "$COPIED" != "marker-$$-$attempt" ] && break
    sleep 0.1
  done
  [ -n "$SELECTED" ] && [ "$COPIED" = "$SELECTED" ] && break
done
eq "⌘C with a selection puts the selected text on the pasteboard" "$COPIED" "$SELECTED"
eq "⌘C leaves the draft empty" "$(composer)" ""

# A waiting permission request: its card has buttons and read-only text, no field, so the typed
# word joins the draft and the request stays waiting.
clear_draft
curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" --data 'permission please' "http://127.0.0.1:$DEBUG_PORT/agent?session=$S" >/dev/null
wait_st '.waiting_count == 1' 20
type_key hello "" window
wait_st --arg p "$P" '.panes[$p].composer == "hello"' 5
eq "hello typed with a permission request waiting lands in the draft" "$(composer)" "hello"
eq "the typed word did not answer the request" "$(st '.waiting_count')" "1"
app_post "/perm?id=$(st '.perms[0].id')&choice=$(st '.perms[0].options[0].id')" >/dev/null
wait_st '.waiting_count == 0' 10

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
