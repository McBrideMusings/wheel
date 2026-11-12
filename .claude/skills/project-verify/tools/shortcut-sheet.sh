#!/usr/bin/env bash
# § Shortcut sheet (wheel-e916.14): ⌘/ opens it, ⌘/ and Esc close it, it lists the keys the keymap
# gives each action, and a `keybind` unbind empties a row. Drives the real Mac app through its
# debug server against this script's own daemon.
# WHEEL_SHOTS=<dir> keeps the window snapshots (set WHEEL_APPEARANCE=light|dark for the theme).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"
LOOK="${WHEEL_APPEARANCE:-light}"

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
mkdir -p "$SCR/alpha"
w space new "$SCR/alpha" >/dev/null
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 1)' 20

press() { app_post "/key?keys=$(printf %s "$1" | sed 's/+/%2B/g')" >/dev/null; }
shot() { app_post "/window/snapshot?path=$SHOTS/shortcuts-$1-$LOOK.png" >/dev/null && echo "     shot $SHOTS/shortcuts-$1-$LOOK.png"; }
keys_of() { st --arg id "$1" '[.shortcuts.groups[].rows[] | select(.id==$id)][0].keys'; }

eq "the sheet starts closed" "$(st .shortcuts.open)" "false"
press cmd+slash
wait_st '.shortcuts.open == true'
eq "⌘/ opens the sheet" "$(st .shortcuts.open)" "true"
eq "its groups" "$(st -c '[.shortcuts.groups[].name]')" '["Panes","Spaces","Windows","Agent"]'
sleep 0.6
shot open

eq "Split Right ⌘D" "$(keys_of split-right)" "⌘D"
eq "Split Down ⇧⌘D" "$(keys_of split-down)" "⇧⌘D"
eq "Close Pane ⌘W" "$(keys_of close-pane)" "⌘W"
eq "Previous Pane ⌘[" "$(keys_of focus-previous)" "⌘["
eq "Next Pane ⌘]" "$(keys_of focus-next)" "⌘]"
eq "Pane Left ⌥⌘←" "$(keys_of focus-left)" "⌥⌘←"
eq "Pane Down ⌥⌘↓" "$(keys_of focus-down)" "⌥⌘↓"
eq "Previous Space ⇧⌘[" "$(keys_of previous-space)" "⇧⌘["
eq "Next Space ⇧⌘]" "$(keys_of next-space)" "⇧⌘]"
eq "Space 1 ⌘1" "$(keys_of space-1)" "⌘1"
eq "Space 8 ⌘8" "$(keys_of space-8)" "⌘8"
eq "Zoom Pane ⇧⌘↩" "$(keys_of zoom-pane)" "⇧⌘↩"
eq "Command Palette ⇧⌘P" "$(keys_of command-palette)" "⇧⌘P"
eq "Keyboard Shortcuts ⌘/" "$(keys_of shortcuts)" "⌘/"

press cmd+slash
wait_st '.shortcuts.open == false'
eq "⌘/ closes the sheet" "$(st .shortcuts.open)" "false"
press cmd+slash
wait_st '.shortcuts.open == true'
press escape
wait_st '.shortcuts.open == false'
eq "Esc closes the sheet" "$(st .shortcuts.open)" "false"

expect_exit 0 "unbind Split Down" w config set keybind "cmd+shift+d=unbind" --machine
wait_st '.keymap["split-down"] == null'
press cmd+slash
wait_st '.shortcuts.open == true'
eq "an unbound Split Down shows no keys" "$(keys_of split-down)" ""
eq "Split Right keeps its key" "$(keys_of split-right)" "⌘D"
sleep 0.6
shot unbound
press escape
wait_st '.shortcuts.open == false'

expect_exit 0 "unset puts the built-in keys back" w config unset keybind --machine
APPLOG="$(st .log_path)"
check "the app log has the sheet opening" grep -q "shortcut sheet: opened" "$APPLOG"
check "the app log has the sheet closing" grep -q "shortcut sheet: closed" "$APPLOG"
finish
