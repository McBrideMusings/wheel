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
eq "the header says the server is running, with no remote listener set" "$(st .menu_bar.status)" "Running · remote access off"
eq "the pairing row says remote access is off, with no remote listener set" "$(st .menu_bar.pairing)" "Remote access is off"

# Send to a window, to the background; a closed window's spaces appear in Background.
app_post "/menubar?action=send_window&space=$S3&window=new" >/dev/null
wait_st '.windows | length == 2'
W2="$(win_of "$S3")"
eq "send to a new window opens it holding only that space" "$(st --arg w "$W2" '.windows[] | select(.id==$w) | .spaces | join(",")')" "$S3"
eq "the source window no longer lists it" "$(st --arg s "$S3" --arg w "$W1" '.windows[] | select(.id==$w) | .spaces | index($s)')" "null"
app_post "/windows?action=place&space=$S4&window=$W2" >/dev/null
app_post "/windows?action=closebutton&window=$W2" >/dev/null
app_post "/windows?action=closeanswer&answer=background" >/dev/null
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
eq "an approval in a space no window shows is counted" "$(st .menu_bar.approvals)" "1"
eq "and 1 on the Dock icon" "$(st .menu_bar.dock_badge)" "1"
eq "the popover row counts it against its space" "$(st -c '.menu_bar.waiting')" "{\"$S1\":1}"
# The row draws one dot per agent, worst first (wheel-5w33): a needs-you agent and a working agent make two.
PANE2="$(w pane split "$PANE" right)"
w pane start "$PANE2" agent --message hello >/dev/null
bg w agent send "$PANE2" "work slowly 600"
wait_st ".menu_bar.rows[] | select(.space==\"$S1\") | .dots.marks == [\"needs_you\",\"working\"]" 20
eq "a needs-you and a working agent show two dots on the row, worst first" "$(st -c --arg s "$S1" '.menu_bar.rows[] | select(.space==$s) | .dots')" '{"marks":["needs_you","working"],"more":0}'
PERM="$(perm_id)"
w perm answer "$PERM" allow >/dev/null
wait_st '.perms | length == 0'
eq "answering clears the count" "$(st .menu_bar.approvals)" "0"
eq "and the Dock badge" "$(st .menu_bar.dock_badge)" "null"

# The status item's badge counts the spaces running with no window; the rows carry the place buttons.
eq "the badge counts the two background spaces" "$(st .menu_bar.badge)" "2"
ICON="$SCR/icon.png"
app_post "/menubar?action=icon&path=$ICON" >/dev/null
check "the status item's icon image is written" test -s "$ICON"
eq "every row has 4 buttons for 1 window (N+3)" "$(st -c '[.menu_bar.rows[].buttons | length] | unique')" "[4]"
eq "the buttons run window, new window, little window, background" "$(st -c '.menu_bar.rows[0].buttons | map(.id | split(":")[0])')" '["window","new-window","little","background"]'
eq "the window's button carries its number" "$(st -c '.menu_bar.rows[0].buttons[0].number')" "1"
eq "the window's group is numbered 1" "$(st -c '.menu_bar.windows[0].number')" "1"
eq "a space in the window lights only the window button" "$(st -r --arg s "$S4" '.menu_bar.rows[] | select(.space==$s) | .buttons | map(select(.lit) | .id) | join(",")')" "window:$W1"
eq "a background space lights only Background" "$(st -r --arg s "$S1" '.menu_bar.rows[] | select(.space==$s) | .buttons | map(select(.lit) | .id) | join(",")')" "background"
eq "its help says it is in the background" "$(st -r --arg s "$S1" '.menu_bar.rows[] | select(.space==$s) | .buttons[-1].help')" "In the background"
app_post "/menubar?action=hover&space=$S4" >/dev/null
eq "hovering a row marks it" "$(st .menu_bar.hover)" "$S4"
app_post "/menubar?action=hover&space=" >/dev/null
eq "moving off clears it" "$(st .menu_bar.hover)" "null"

# Every button moves the space where it says, read back through wheel window ls.
app_post "/menubar?action=place&space=$S4&button=background" >/dev/null
eq "Background button: wheel window ls lists it in the background" "$(w window ls --json | jq -r --arg s "$S4" '.background | map(.id) | index($s) | type')" "number"
app_post "/menubar?action=place&space=$S4&button=window:$W1" >/dev/null
eq "window button: wheel window ls lists it in the window" "$(w window ls --json | jq -r --arg s "$S4" --arg w "$W1" '.windows[] | select(.id==$w) | .spaces | map(.id) | index($s) | type')" "number"
app_post "/menubar?action=place&space=$S4&button=new-window" >/dev/null
eq "New Window button: a second window holds it alone" "$(w window ls --json | jq -r --arg s "$S4" '[.windows[] | select(.spaces | map(.id) == [$s])] | length')" "1"
W2="$(win_of "$S4")"
app_post "/menubar?action=place&space=$S4&button=little" >/dev/null
eq "Little Window button on a windowed space: an owned little window" "$(w window ls --json | jq -r --arg s "$S4" '.little[] | select(.space.id==$s) | .owner')" "$W2"
eq "the row is dimmed and lights only Little Window" "$(st -c --arg s "$S4" '.menu_bar.rows[] | select(.space==$s) | [.dimmed, (.buttons | map(select(.lit) | .id))]')" '[true,["little"]]'
app_post "/menubar?action=place&space=$S4&button=window:$W2" >/dev/null
eq "the owner's window button brings it back" "$(w window ls --json | jq '.little | length')" "0"
app_post "/menubar?action=place&space=$S1&button=little" >/dev/null
eq "Little Window button on a background space: an ownerless one" "$(w window ls --json | jq -r --arg s "$S1" '.little[] | select(.space.id==$s) | .owner')" "null"
eq "the Background group still lists it, with the little-window mark" "$(st -c --arg s "$S1" '.menu_bar.rows[] | select(.space==$s) | [.group, .little_mark, .dimmed]')" '["background",true,false]'
eq "and the badge still counts it" "$(st .menu_bar.badge)" "2"
app_post "/menubar?action=place&space=$S1&button=background" >/dev/null
eq "Background button on it closes the little window" "$(w window ls --json | jq '.little | length')" "0"
app_post "/space/menu?space=$S1&item=little" >/dev/null
eq "the space menu works on a background row" "$(w window ls --json | jq '.little | length')" "1"
app_post "/windows?action=littleback&space=$S1" >/dev/null

# The popover itself opens from the status item.
app_post "/menubar?action=open" >/dev/null
check "the status item opens the popover" wait_st '.menu_bar.open' 5
app_post "/menubar?action=close" >/dev/null
check "and closes it" wait_st '.menu_bar.open | not' 5
finish
