#!/usr/bin/env bash
# § 4 Windows show sessions (ADR-0014, wheel-zpa4.1.5): a window shows one session and one of its spaces,
# several windows may show the same session, a session no window shows is the background, closing the
# last window of a session asks, and the windows come back at launch.
# Drives the real Mac app through its debug server against this script's own daemon.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
IDS=($(mkspaces 6))
S1=${IDS[0]} S2=${IDS[1]} S3=${IDS[2]} S4=${IDS[3]} S5=${IDS[4]} S6=${IDS[5]}
N1="$(w session ls --json | jq -r '.[0].id')"

launch_app
check "the app connects at launch with its window open" wait_st '.connection == "connected"' 20
wait_st '.windows[0].spaces | length == 6' 10
eq "first launch: one window" "$(st '.windows | length')" "1"
eq "first launch: it shows the first session" "$(st '.windows[0].session')" "$N1"
eq "first launch: it lists the session's six spaces" "$(st '.windows[0].spaces | length')" "6"

# A second session holding S4 S5 S6, made and filled with the CLI: no window shows it, so it is the background.
N2="$(w session new Review)"
for s in "$S4" "$S5" "$S6"; do w session move "$s" "$N2" >/dev/null; done
wait_st '.sessions | length == 2' 10
wait_st '.windows[0].spaces | length == 3' 10
eq "the first window lists only its session's three spaces" "$(st '.windows[0].spaces | sort | join(",")')" "$(printf '%s\n' "$S1" "$S2" "$S3" | sort | paste -sd, -)"
eq "a session no window shows is the background" "$(st '.background_sessions | join(",")')" "$N2"
eq "its three spaces are the background spaces" "$(st '.background | sort | join(",")')" "$(printf '%s\n' "$S4" "$S5" "$S6" | sort | paste -sd, -)"

# Two windows show Review, each its own space of it.
W1="$(st '.windows[0].id')"
W2="$(app_post "/windows?action=new&session=$N2&space=$S5" | jq -r .window)"
wait_st '.windows | length == 2' 10
eq "a new window on a session shows that session" "$(st --arg w "$W2" '.windows[] | select(.id==$w) | .session')" "$N2"
eq "a new window opened at a space shows it" "$(st --arg w "$W2" '.windows[] | select(.id==$w) | .active')" "$S5"
app_post "/windows?action=session&window=$W1&session=$N2" >/dev/null
wait_st ".windows[] | select(.id==\"$W1\") | .session == \"$N2\"" 10
eq "two windows show the same session" "$(st --arg n "$N2" '[.windows[] | select(.session==$n)] | length')" "2"
eq "each lists the session's three spaces" "$(st --arg n "$N2" '[.windows[] | select(.session==$n) | .spaces | length] | join(",")')" "3,3"
eq "switching the first window's session shows that session's first space" "$(st --arg w "$W1" '.windows[] | select(.id==$w) | .active')" "$S4"
eq "the second window keeps its own space" "$(st --arg w "$W2" '.windows[] | select(.id==$w) | .active')" "$S5"
eq "no session is in the background while two windows show Review and none shows the first" "$(st '.background_sessions | join(",")')" "$N1"
BEFORE="$(st -c '[.windows[] | {id, session, active}]')"

# Quit and reopen: the same windows, sessions and active spaces.
quit_app
check "the app quit" test -z "$(app_pid)"
launch_app
wait_st '.windows | length == 2' 20
eq "after a relaunch the windows, their sessions and active spaces are as they were" "$(st -c '[.windows[] | {id, session, active}]')" "$BEFORE"

# Closing a window whose session another window shows closes it without asking.
app_post "/windows?action=closebutton&window=$W2" >/dev/null
wait_st '.windows | length == 1' 10
eq "the first window stays" "$(st '.windows[0].id')" "$W1"
eq "Review is still shown, so the first session alone is background" "$(st '.background_sessions | join(",")')" "$N1"

# The last window of a session closes without asking too, and the session keeps running.
app_post "/windows?action=closebutton&window=$W1" >/dev/null
check "the last window of a session closes at once" wait_st '.windows | length == 0' 10
eq "the app and its connection stay" "$(st .connection)" "connected"
eq "both sessions are background sessions" "$(st '.background_sessions | sort | join(",")')" "$(printf '%s\n' "$N1" "$N2" | sort | paste -sd, -)"
eq "every space still runs in wheeld" "$(w space ls | jq '.spaces | length')" "6"
app_post "/windows?action=reopen" >/dev/null
wait_st '.windows | length == 1' 10
eq "the Dock icon opens a window on a background session" "$(st '.windows[0].session != null')" "true"

# Closing a session no window shows closes its spaces and their panes.
app_post "/windows?action=session&window=$(st '.windows[0].id')&session=$N1" >/dev/null
wait_st '.windows[0].session == "'"$N1"'"' 10
w session close "$N2" >/dev/null
wait_st '.sessions | length == 1' 10
eq "closing the session closes it in wheeld" "$(w session ls --json | jq -r --arg n "$N2" '[.[] | select(.id==$n)] | length')" "0"
eq "and its three spaces" "$(w space ls | jq '.spaces | length')" "3"

# A session closed by the CLI while a window shows it: the window shows the first session.
N3="$(w session new Third)"
wait_st '.sessions | length == 2' 10
app_post "/windows?action=session&window=$(st '.windows[0].id')&session=$N3" >/dev/null
wait_st '.windows[0].session == "'"$N3"'"' 10
w session close "$N3" >/dev/null
wait_st '.sessions | length == 1' 10
eq "a window whose session the CLI closed shows the first session" "$(st '.windows[0].session')" "$N1"

# Restore off: one window.
app_post "/windows?action=new&session=$N1" >/dev/null
quit_app
defaults write "$PREFS_SUITE" restoreWindows -bool false
launch_app
wait_st '.spaces | length == 3' 20
eq "with restore off, launch opens one window" "$(st '.windows | length')" "1"
eq "the state reports the setting" "$(st .restore_windows)" "false"

w space close "$S1" >/dev/null
eq "a space closed in wheeld leaves the window's tabs" "$(wait_st '.windows[0].spaces | length == 2' 5 && echo yes)" "yes"
w space new "$SCR/proj1" >/dev/null
eq "a space made while a window is open joins the window's session" "$(wait_st '.windows[0].spaces | length == 3' 5 && echo yes)" "yes"

has_log "Mac app attached" "the daemon saw the app connect"
finish
