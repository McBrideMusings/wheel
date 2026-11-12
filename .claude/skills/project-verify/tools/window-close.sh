#!/usr/bin/env bash
# § 4 Closing a main window: the red light, ⇧⌘W and `wheel window close` close it without asking,
# even the last window showing a session; the session keeps every space running in the background
# and its little windows stay, owned by none.
# Drives the real Mac app and the CLI against this script's own daemon.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
IDS=($(mkspaces 4))
S1=${IDS[0]} S2=${IDS[1]} S3=${IDS[2]} S4=${IDS[3]}
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 4)' 20
A="$(st '.windows[0].id')"
B="$(w window new --name Second --space "$S4")"
wait_st '.windows | length == 2' 10
app_post "/windows?action=little&space=$S2" >/dev/null
wait_st '.little_windows | length == 1 and .little_windows[0].open' 5

# The red light on a window whose session another window shows closes it; the session stays shown.
app_post "/windows?action=closebutton&window=$A" >/dev/null
check "the window closes without asking" wait_st '[.windows[] | select(.id=="'"$A"'")] | length == 0' 5
eq "the other window stays" "$(st '.windows[0].id')" "$B"
eq "no session went to the background" "$(st '.background_sessions | length')" "0"
eq "the little window stays open" "$(st '.little_windows | length')" "1"

# `wheel window close` on the last window closes it too; the session runs on in the background.
SESSION="$(st '.windows[0].session')"
expect_exit 0 "window close closes the last window" w window close "$B"
check "no window is left" wait_st '.windows | length == 0' 10
eq "its session is a background session" "$(st '.background_sessions | join(",")')" "$SESSION"
eq "the little window stays open" "$(st '.little_windows | length')" "1"
eq "and belongs to none" "$(w window ls --json | jq -r '.little[0].owner')" "null"
eq "the app and its connection stay" "$(st .connection)" "connected"
eq "every space still runs in wheeld" "$(w space ls | jq '.spaces | length')" "4"

# A new window shows the session again.
app_post "/windows?action=reopen" >/dev/null
wait_st '.windows | length == 1' 10
eq "the reopened window shows the session" "$(st '.windows[0].session')" "$SESSION"
finish
