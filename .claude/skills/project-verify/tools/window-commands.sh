#!/usr/bin/env bash
# § 4 Renaming and `wheel window`: space names are wheeld's, window names the Mac app's, and the
# window commands reach the app through the daemon. Drives the real Mac app and the CLI.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
for n in plex-db-ex reddit-poker wheel; do mkdir -p "$SCR/$n"; done
A="$(w space new "$SCR/plex-db-ex")" B="$(w space new "$SCR/reddit-poker")" C="$(w space new "$SCR/wheel")"

# With no Mac app attached the window commands fail and say so; space commands do not need it.
expect_exit_out 6 "no Mac app attached" "window ls fails with no app" w window ls
expect_exit_out 6 "no Mac app attached" "space move fails with no app" w space move "$A" background
expect_exit 0 "space rename needs no Mac app" w space rename "$A" "plex"
eq "the space is renamed" "$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | .name')" "plex"
expect_exit 0 "a space is renamed by its name" w space rename plex "plex-db"
eq "the new name is found by name" "$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | .name')" "plex-db"
expect_exit_out 6 "needs a name" "an empty name is refused by the daemon" w space rename "$A" "  "
expect_exit_out 1 "no space nope; the spaces are" "an unknown space says which spaces exist" w space rename nope x
restart_daemon
eq "the name survives a daemon restart" "$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | .name')" "plex-db"

launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 3)' 20
W1="$(st '.windows[0].id')"

# `wheel window`.
NEW="$(w window new --name Reviews --space "$B")"
check "window new prints the new window's id" test -n "$NEW"
eq "the new window is named and holds that space" "$(st --arg w "$NEW" '.windows[] | select(.id==$w) | .name + " " + (.spaces | join(","))')" "Reviews $B"
expect_out "Reviews .*1 space: reddit-poker \\($B\\)" "window ls lists the name, count and spaces" w window ls
expect_out "background: $" "and an empty background" w window ls
eq "window ls --json has both windows" "$(w window ls --json | jq '.windows | length')" "2"
expect_exit 0 "space move to a window by its name" w space move "$C" Reviews
eq "the space is in that window only" "$(st --arg s "$C" '[.windows[] | select(.spaces | index($s)) | .name] | join(",")')" "Reviews"
expect_exit 0 "space move to the background" w space move wheel background
eq "the menu bar item lists it under Background" "$(st '.menu_bar.background | join(",")')" "wheel"
expect_out "background: wheel \\($C\\)" "and window ls does" w window ls
expect_exit_out 6 "no window nope" "window rename of a missing window names it" w window rename nope x
expect_exit 0 "window rename by name" w window rename Reviews "Code review"
eq "the window has its new name" "$(st --arg w "$NEW" '.windows[] | select(.id==$w) | .name')" "Code review"
expect_exit_out 6 "no space nope" "a missing space is named" w space move nope background
expect_exit 0 "window close asks in a sheet on the window" w window close "Code review"
eq "the sheet is showing for that window" "$(st .window_close.window)" "$NEW"
app_post "/windows?action=closeanswer&answer=background" >/dev/null
wait_st '.windows | map(select(.id=="'"$NEW"'")) | length == 0' 10
eq "the closed window's space is in the background" "$(st '.background | length')" "2"

# Renaming in the app: a space through the daemon, a window in the app's own layout.
app_post "/rename?kind=space&id=$B&action=begin" >/dev/null
eq "beginning a space rename starts editing it" "$(st .renaming.space)" "$B"
app_post "/rename?kind=space&id=$B&action=commit&name=" >/dev/null
eq "an empty name is refused with its reason shown" "$(st --arg s "$B" '.renaming.errors[$s]')" "A space needs a name."
eq "and the field stays open" "$(st .renaming.space)" "$B"
app_post "/rename?kind=space&id=$B&action=commit&name=reviews" >/dev/null
wait_st '.renaming.space == null'
eq "Return renames the space in wheeld" "$(w space ls | jq -r --arg s "$B" '.spaces[] | select(.id==$s) | .name')" "reviews"
eq "and the app shows it" "$(st --arg s "$B" '.spaces[] | select(.id==$s) | .name')" "reviews"
app_post "/rename?kind=space&id=$B&action=begin" >/dev/null
app_post "/rename?kind=space&id=$B&action=cancel" >/dev/null
eq "Escape leaves the name as it was" "$(w space ls | jq -r --arg s "$B" '.spaces[] | select(.id==$s) | .name')" "reviews"

app_post "/rename?kind=window&id=$W1&action=commit&name=Work" >/dev/null
eq "a window's name shows in the state" "$(st --arg w "$W1" '.windows[] | select(.id==$w) | .name')" "Work"
quit_app
launch_app
wait_st '.windows | length >= 1'
eq "the window's name is the same after relaunch" "$(st --arg w "$W1" '.windows[] | select(.id==$w) | .name')" "Work"
app_post "/rename?kind=window&id=$W1&action=commit&name=" >/dev/null
eq "an empty name clears it" "$(st --arg w "$W1" '.windows[] | select(.id==$w) | .name')" "null"
ACTIVE="$(st --arg w "$W1" '.windows[] | select(.id==$w) | .active')"
eq "and the title is the active space's name again" "$(st --arg w "$W1" '.windows[] | select(.id==$w) | .title')" "$(st --arg s "$ACTIVE" '.spaces[] | select(.id==$s) | .name')"
finish
