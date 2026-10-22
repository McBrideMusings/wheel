#!/usr/bin/env bash
# § 4 Menu bar item: the popover's windows and Background card, the send buttons, the approval badge.
# Drives the real Mac app through its debug server against this script's own daemon.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
IDS=($(mkspaces 4))
S1=${IDS[0]} S2=${IDS[1]} S3=${IDS[2]} S4=${IDS[3]}
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 4)' 20
W1="$(st '.windows[0].id')"

eq "the menu bar lists the one window with its four spaces" "$(st -c '.menu_bar.windows | map(.spaces | length)')" "[4]"
eq "the Background card is empty" "$(st '.menu_bar.background | length')" "0"
eq "the header says the server is running" "$(st .menu_bar.status)" "Running"
eq "the pairing row says remote access is off, with no remote listener set" "$(st .menu_bar.pairing)" "Remote access is off"

# Send to a window, to the background; a closed window's spaces appear in Background.
app_post "/menubar?action=send_window&space=$S3&window=new" >/dev/null
wait_st '.windows | length == 2'
W2="$(win_of "$S3")"
eq "send to a new window opens it holding only that space" "$(st --arg w "$W2" '.windows[] | select(.id==$w) | .spaces | join(",")')" "$S3"
eq "the source window no longer lists it" "$(st --arg s "$S3" --arg w "$W1" '.windows[] | select(.id==$w) | .spaces | index($s)')" "null"
app_post "/windows?action=place&space=$S4&window=$W2" >/dev/null
app_post "/windows?action=closebutton&window=$W2" >/dev/null
wait_st '.windows | length == 1'
eq "closing a window lists its spaces under Background" "$(st '.menu_bar.background | length')" "2"
app_post "/menubar?action=send_window&space=$S4&window=$W1" >/dev/null
eq "show in a window moves it out of the Background card" "$(st '.menu_bar.background | length')" "1"
eq "the window card lists it" "$(st --arg w "$W1" '.windows[] | select(.id==$w) | .spaces | index("'"$S4"'") | type')" "number"
app_post "/menubar?action=send_background&space=$S1" >/dev/null
eq "send to background moves the space into the Background card" "$(st '.menu_bar.background | length')" "2"
eq "and the window no longer lists it" "$(st --arg s "$S1" '[.windows[].spaces[]] | index($s)')" "null"
eq "wheeld keeps every space running" "$(w space ls | jq '.spaces | length')" "4"

# An approval waiting in a space no window shows badges the item and the Dock icon.
PANE="$(w space ls | jq -r --arg s "$S1" '.spaces[] | select(.id==$s) | .focus')"
w pane start "$PANE" agent >/dev/null
bg w agent send "$PANE" "permission please"
wait_st '.perms | length == 1'
eq "an approval in a space no window shows puts 1 on the menu bar item" "$(st .menu_bar.badge)" "1"
eq "and 1 on the Dock icon" "$(st .menu_bar.dock_badge)" "1"
eq "the popover row counts it against its space" "$(st -c '.menu_bar.waiting')" "{\"$S1\":1}"
PERM="$(perm_id)"
w perm answer "$PERM" allow >/dev/null
wait_st '.perms | length == 0'
eq "answering clears the menu bar badge" "$(st .menu_bar.badge)" "0"
eq "and the Dock badge" "$(st .menu_bar.dock_badge)" "null"

# The popover itself opens from the status item.
app_post "/menubar?action=open" >/dev/null
check "the status item opens the popover" wait_st '.menu_bar.open' 5
app_post "/menubar?action=close" >/dev/null
check "and closes it" wait_st '.menu_bar.open | not' 5
finish
