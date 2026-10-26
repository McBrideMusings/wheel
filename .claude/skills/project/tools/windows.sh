#!/usr/bin/env bash
# § 4 Windows: windows hold spaces, closing sends them to the background, the layout comes back at launch.
# Drives the real Mac app through its debug server against this script's own daemon.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
IDS=($(mkspaces 6))
S1=${IDS[0]} S2=${IDS[1]} S3=${IDS[2]} S4=${IDS[3]} S5=${IDS[4]} S6=${IDS[5]}

launch_app
check "the app connects at launch with its window open" wait_st '.connection == "connected"' 20
wait_st '.windows[0].spaces | length == 6' 10
eq "first launch: one window holds all six spaces" "$(st '.windows | length')" "1"
eq "first launch: the window lists every space" "$(st '.windows[0].spaces | length')" "6"

# Two windows with three spaces each, a different space shown in each.
W1="$(st '.windows[0].id')"
app_post "/windows?action=new&spaces=$S4,$S5,$S6" >/dev/null
wait_st '.windows | length == 2' 10
W2="$(win_of "$S4")"
app_post "/windows?action=place&space=$S5&window=$W2&at=0" >/dev/null
app_post "/windows?action=place&space=$S2&window=$W1&at=0" >/dev/null
eq "window 1 holds s2, s1, s3 with s2 shown" "$(st --arg w "$W1" '.windows[] | select(.id==$w) | (.spaces|join(",")) + " " + .active')" "$S2,$S1,$S3 $S2"
eq "window 2 holds s5, s4, s6 with s5 shown" "$(st --arg w "$W2" '.windows[] | select(.id==$w) | (.spaces|join(",")) + " " + .active')" "$S5,$S4,$S6 $S5"
BEFORE="$(st -c '[.windows[] | {id, spaces, active}]')"

# Quit and reopen: the same two windows, spaces and active spaces.
quit_app
check "the app quit" test -z "$(app_pid)"
launch_app
wait_st '.windows | length == 2' 20
eq "after a relaunch the windows, their spaces and active spaces are as they were" "$(st -c '[.windows[] | {id, spaces, active}]')" "$BEFORE"
eq "quitting did not send any space to the background" "$(st '.background | length')" "0"

# Close one window with its close button: the other stays, its three spaces are background.
app_post "/windows?action=closebutton&window=$W2" >/dev/null
wait_st '.windows | length == 1' 10
eq "closing a window leaves the other open" "$(st '.windows[0].id')" "$W1"
eq "the closed window's three spaces are background spaces" "$(st '.background | sort | join(",")')" "$(printf '%s\n' "$S4" "$S5" "$S6" | sort | paste -sd, -)"
eq "the spaces still run in wheeld" "$(w space ls | jq '.spaces | length')" "6"

# Close the last window: the app and its connection stay.
app_post "/windows?action=closebutton&window=$W1" >/dev/null
wait_st '.windows | length == 0' 10
eq "closing the last window leaves no window" "$(st '.windows | length')" "0"
eq "the app is still running" "$(test -n "$(app_pid)" && echo yes)" "yes"
eq "the connection to wheeld is still up" "$(st .connection)" "connected"
eq "all six spaces are background spaces" "$(st '.background | length')" "6"
app_post "/windows?action=reopen" >/dev/null
wait_st '.windows | length == 1' 10
eq "the Dock icon opens a window holding the background spaces" "$(st '.windows[0].spaces | length')" "6"

# A space deleted with the CLI while the app is closed is in no window at the next launch.
quit_app
w space close "$S2" >/dev/null
launch_app
wait_st '.spaces | length == 5' 20
eq "a space closed while the app was away is in no window" "$(st --arg s "$S2" '[.windows[].spaces[], .background[]] | index($s)')" "null"
eq "the other five spaces are still placed" "$(st '[.windows[].spaces[], .background[]] | length')" "5"

# Restore off: one window holds every space.
app_post "/windows?action=new&spaces=$S3" >/dev/null
quit_app
defaults write "$PREFS_SUITE" restoreWindows -bool false
launch_app
wait_st '.spaces | length == 5' 20
eq "with restore off, launch opens one window" "$(st '.windows | length')" "1"
eq "with restore off, that window holds every space" "$(st '.windows[0].spaces | length')" "5"
eq "with restore off, nothing is in the background" "$(st '.background | length')" "0"
eq "the state reports the setting" "$(st .restore_windows)" "false"

w space close "$S1" >/dev/null
eq "a space closed in wheeld leaves its window" "$(wait_st '.windows[0].spaces | length == 4' 5 && echo yes)" "yes"
w space new "$SCR/proj1" >/dev/null
eq "a space made while a window is open opens in the focused window" "$(wait_st '.windows[0].spaces | length == 5' 5 && echo yes)" "yes"

has_log "Mac app attached" "the daemon saw the app connect"
finish
