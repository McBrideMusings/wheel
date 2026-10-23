#!/usr/bin/env bash
# § 4 Background spaces (ADR-0014): `wheel space new --background` makes a space that runs with no
# window and leaves the screen alone; the Mac app closes a background space once its last pane
# closes, and nothing else. Drives the real Mac app through its debug server against this
# script's own daemon.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
S1="$(mkspaces 1)"
launch_app
check "the app connects with the one space in its window" wait_st ".windows[0].spaces == [\"$S1\"]" 20
APPLOG="$(st .log_path)"
pane_of() { w space ls | jq -r --arg s "$1" '.spaces[] | select(.id == $s) | .focus'; }
gone() { [ "$(w space ls | jq --arg s "$1" '[.spaces[] | select(.id == $s)] | length')" = 0 ]; }

# A background space: in no window, and the screen still shows what it showed.
B="$(w space new "$SCR/proj1" --name "proj1 swarm" --background)"
check "the new space is a background space" wait_st ".background | index(\"$B\") != null" 10
eq "the window still holds only the first space" "$(st -c '.windows[0].spaces')" "[\"$S1\"]"
eq "the window still shows the first space" "$(st '.windows[0].active')" "$S1"
eq "wheeld's active space is still the first" "$(w space ls | jq -r .active)" "$S1"
has_log "space created.*focus=false" "the daemon logs the space made without focus"
check "the app logs it opened in the background" grep -q "new space $B opened in the background" "$APPLOG"

# Its one empty pane is not a closed last pane.
sleep 2
eq "a fresh background space with one empty pane stays" "$(gone "$B" && echo gone || echo kept)" "kept"

# Two panes, close one: still there. Close the last: the app closes the space.
P1="$(pane_of "$B")"
w pane start "$P1" terminal >/dev/null
P2="$(w pane split "$P1" right --background)"
sleep 1
eq "a background split leaves the space in the background" "$(st ".background | index(\"$B\") != null")" "true"
w pane close "$P2" >/dev/null
sleep 1
eq "closing one of two panes keeps the background space" "$(gone "$B" && echo gone || echo kept)" "kept"
w pane close "$P1" >/dev/null
check "closing its last pane closes the background space" poll 10 gone "$B"
check "the app logs why it closed it" grep -q "background space $B lost its last pane; closing it" "$APPLOG"

# A background space whose one pane only became empty keeps it.
B2="$(w space new "$SCR/proj1" --name "kind check" --background)"
wait_st ".background | index(\"$B2\") != null" 10
P3="$(pane_of "$B2")"
w pane start "$P3" terminal >/dev/null
w pane kind "$P3" empty >/dev/null
sleep 2
eq "a background space whose pane became empty stays" "$(gone "$B2" && echo gone || echo kept)" "kept"

# A space in a window keeps one empty pane when its last pane closes.
P4="$(pane_of "$S1")"
w pane start "$P4" terminal >/dev/null
w pane close "$P4" >/dev/null
sleep 2
eq "a space in a window survives its last pane closing" "$(gone "$S1" && echo gone || echo kept)" "kept"
eq "and holds one new empty pane" "$(w space ls | jq -r --arg s "$S1" '.spaces[] | select(.id == $s) | .root.kind')" "empty"

[ "$FAIL" = 0 ] || grep -E "background|lost its last|new space|closing" "$APPLOG" | tail -20
finish
