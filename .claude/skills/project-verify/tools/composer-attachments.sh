#!/usr/bin/env bash
# The Mac composer's attachments: a file or a pasted image is a chip with an X above the text, not
# a path in the text box. Files stay where they are; an image with no file is saved once and its
# copy goes when the chip is removed; the paths go out after the typed text at send.
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

chips() { st --arg p "$P" '[.panes[$p].attachments[].name] | join(",")'; }
composer() { st --arg p "$P" '.panes[$p].composer'; }
count() { st --arg p "$P" '.panes[$p].attachments | length'; }
post() { curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" "http://127.0.0.1:$DEBUG_PORT$1" "${@:2}" >/dev/null; }
paste() { sleep 0.5; post "/agent/paste?agent=$S"; }

printf 'hello' > "$SCR/notes with space.txt"

# A file dropped on the composer.
post "/agent/attach?agent=$S&path=$SCR/notes%20with%20space.txt"
wait_st --arg p "$P" '.panes[$p].attachments | length == 1' 5
eq "a dropped file is a chip named for the file" "$(chips)" "notes with space.txt"
eq "the draft holds no path" "$(composer)" ""

# A pasted file URL.
post "/agent/pasteboard?agent=$S&kind=file&path=$SCR/notes%20with%20space.txt"
paste
wait_st --arg p "$P" '.panes[$p].attachments | length == 2' 5
eq "a pasted file is a second chip" "$(count)" "2"
eq "a pasted file adds no path to the draft" "$(composer)" ""

# The text box takes no drops of its own, so a drop reaches the composer and not the text.
eq "the text box registers no drag types" "$(st --arg p "$P" '.panes[$p].composer_drag_types')" "0"

# A pasted image.
post "/agent/pasteboard?agent=$S&kind=image"
paste
wait_st --arg p "$P" '.panes[$p].attachments | length == 3' 5
eq "a pasted image is a chip with a thumbnail" "$(st --arg p "$P" '.panes[$p].attachments[2].thumbnail')" "true"
IMG="$(st --arg p "$P" '.panes[$p].attachments[2].name')"

# Text on the pasteboard is still a text paste.
post "/agent/pasteboard?agent=$S&kind=text&text=pasted-words"
paste
wait_st --arg p "$P" '.panes[$p].composer == "pasted-words"' 5
eq "pasted text goes in the draft" "$(composer)" "pasted-words"
eq "pasted text adds no chip" "$(count)" "3"

# The X removes a chip.
post "/agent/attach?agent=$S&remove=$IMG"
wait_st --arg p "$P" '.panes[$p].attachments | length == 2' 5
eq "removing the image's chip leaves two" "$(chips)" "notes with space.txt,notes with space.txt"

# Send: the text, then each path in backticks, and the tray empties.
post "/agent?agent=$S" --data 'look at this'
wait_st --arg p "$P" '(.panes[$p].transcript | map(select(startswith("user:"))) | length) > 0' 15
SENT="$(st --arg p "$P" '[.panes[$p].transcript[] | select(startswith("user:"))] | last')"
case "$SENT" in
  *'look at this'*'`'"$SCR/notes with space.txt"'`'*) ok "the message is the text then the path in backticks" ;;
  *) fail "the message is the text then the path in backticks" "$SENT" ;;
esac
eq "the chips clear after the send" "$(count)" "0"
finish
