#!/usr/bin/env bash
# § 4 Two starts on one empty pane (wheel-8zk7): the Mac app sends one `pane_start` and ignores a
# second for the same pane until the first is answered, so the first agent is never replaced and no
# "agent ended" error shows. The daemon's login shell takes a second to answer here, as the real one
# does, so the second start arrives while the first is running.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

printf '#!/bin/sh\nsleep 1\nexec /bin/sh "$@"\n' >"$SCR/slow-sh"
chmod +x "$SCR/slow-sh"
DAEMON_SHELL="$SCR/slow-sh"
use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
mkdir -p "$SCR/alpha"
S="$(w space new "$SCR/alpha")"
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 1)' 20
P="$(st --arg s "$S" '.spaces[] | select(.id==$s) | .focus')"

# Two starts back to back, as a double click sends them.
app_post "/start?pane=$P&kind=agent" >/dev/null
app_post "/start?pane=$P&kind=agent" >/dev/null
wait_st --arg p "$P" '[.spaces[].root | .. | objects | select(.node=="pane" and .id==$p)][0].kind == "agent"' 15
sleep 1

APPLOG="$(st .log_path)"
eq "the app sent one pane_start for the pane" "$(grep -c "\"op\":\"pane_start\",\"pane\":\"$P\"" "$APPLOG")" "1"
eq "the app ignored the second start" "$(grep -c "start $P ignored" "$APPLOG")" "1"
eq "the daemon started one agent in the pane" "$(grep -c "agent started .*pane=$P" "$LOGS/wheeld.log")" "1"
eq "no agent ended" "$(grep -c "agent ended" "$LOGS/wheeld.log")" "0"
eq "no error shows above the grid" "$(st '.layout_error // "none"')" "none"
finish
