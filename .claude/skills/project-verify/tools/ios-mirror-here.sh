#!/usr/bin/env bash
# § iPhone mirror in its source's space (wheel-eg4e.19.8): space `work` holds a terminal named
# `build` and a mirror of it, so the phone's carousel holds both pages side by side. Each page draws
# its own Ghostty surface (one surface draws in one view): the mirror's is an embed hosted by the
# mirror pane, given the screen's replay with the source's or, when it comes up while the source
# already streams, by an attach of its own. Run 1 types on the source page, then swipes to the
# mirror; run 2 opens on the mirror, then swipes to the source. After each swipe the phone logs the
# landed page's surface and the rows it holds, which must be that page's own and hold the typed
# line; `simctl io screenshot` saves the page (WHEEL_SHOTS=<dir>).
#
# Needs WHEEL_IOS_APP: the built Wheel.app for the simulator (WheelPhone scheme), and a pool simulator.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SIM=~/.claude/tools/sim
BUNDLE=com.piercemakes.wheel.phone
UDID=""
ios_cleanup() { [ -n "$UDID" ] && "$SIM" release >/dev/null 2>&1; cleanup; }
trap ios_cleanup EXIT

[ -d "${WHEEL_IOS_APP:-}" ] || { echo "set WHEEL_IOS_APP to the built Wheel.app (WheelPhone, iphonesimulator)" >&2; exit 2; }
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

mkdir -p "$SCR/work"
WORK="$(w space new "$SCR/work")"
w space rename "$WORK" work >/dev/null
P1="$(w space ls | jq -r --arg s "$WORK" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
w pane start "$P1" terminal >/dev/null
w pane rename "$P1" build >/dev/null
TERM_ID="$(w space ls | jq -r --arg p "$P1" '[.spaces[].root | .. | select(.id? == $p)][0].term')"
M="$(w pane mirror "$P1" --space "$WORK")"
check "wheeld made a mirror in the source's own space" test -n "$M"

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
xcrun simctl install "$UDID" "$WHEEL_IOS_APP"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
PLOG="$DATA/Documents/phone.log"
rm -f "$PLOG"
plog() { grep -q -- "$1" "$PLOG" 2>/dev/null; }
launch() { xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1; sleep 1; env "$@" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null; }
shot() { sleep 3; xcrun simctl io "$UDID" screenshot "$SHOTS/ios-mirror-here-$1.png" >/dev/null 2>&1; }

code="$(w remote pair --address 127.0.0.1)"
launch SIMCTL_CHILD_WHEEL_PAIR_JSON="$code" SIMCTL_CHILD_WHEEL_START_SPACE=work
check "the phone pairs" poll 40 plog "connected to wss://127.0.0.1:$RPORT/ws"
check "the mirror draws as its source, titled work · build" poll 20 plog "mirror $M in $WORK shows $P1 as terminal: work · build"
check "work's pages are the source and its mirror" poll 20 plog "active $WORK, panes \[\""
ORDER="$(grep -o "active $WORK, panes \[[^]]*\]" "$PLOG" | tail -1)"
SRC_PAGE=1; MIRROR_PAGE=2
case "$ORDER" in *"[\"$M\""*) SRC_PAGE=2; MIRROR_PAGE=1 ;; esac

# Run 1: the source page attaches first; the mirror's surface comes up while the term already streams.
xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1; : >"$PLOG"
launch SIMCTL_CHILD_WHEEL_START_SPACE=work SIMCTL_CHILD_WHEEL_START_PAGE="$SRC_PAGE" SIMCTL_CHILD_WHEEL_TYPE='echo here-$((6*7))' SIMCTL_CHILD_WHEEL_SWIPE="$MIRROR_PAGE"
check "case 1: the source page attaches to its terminal" poll 30 plog "attached to $TERM_ID"
check "case 1: text typed on the source page reaches the shell" w term read "$TERM_ID" --until here-42 --timeout 20
check "case 1: the phone swipes to the mirror's page" poll 30 plog "page $M:"
check "case 1: the mirror's page draws a surface of its own" poll 20 plog "mirror embed terminal $M shows $TERM_ID"
# Either both pages' surfaces were up for the first attach, or the mirror's came later and asked again.
check "case 1: the mirror's surface took a replay" poll 20 sh -c "grep -q 'attached to $TERM_ID, replay [0-9]* bytes to 2 of 2 surfaces' '$PLOG' || grep -q 'attaching to $TERM_ID for a new surface' '$PLOG'"
check "case 1: the mirror's own surface holds the typed line" poll 20 plog "screen $M: embed .*here-42"
shot mirror
cp "$PLOG" "$SHOTS/ios-mirror-here-run1.log"

# Run 2: the mirror's page comes up first; the source page's surface then needs the replay.
xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1; : >"$PLOG"
launch SIMCTL_CHILD_WHEEL_START_SPACE=work SIMCTL_CHILD_WHEEL_START_PAGE="$MIRROR_PAGE" SIMCTL_CHILD_WHEEL_SWIPE="$SRC_PAGE"
check "case 2: the mirror's page draws a surface of its own" poll 30 plog "mirror embed terminal $M shows $TERM_ID"
check "case 2: the phone swipes to the source's page" poll 30 plog "page $P1:"
# Either both surfaces were up for the first attach, or the source's came later and asked again.
check "case 2: the source page's surface took a replay" poll 20 sh -c "grep -q 'attached to $TERM_ID, replay [0-9]* bytes to 2 of 2 surfaces' '$PLOG' || grep -q 'attaching to $TERM_ID for a new surface' '$PLOG'"
check "case 2: the source page's own surface holds the typed line" poll 20 plog "screen $P1: own .*here-42"
shot source
cp "$PLOG" "$SHOTS/ios-mirror-here-run2.log"

[ "$FAIL" -eq 0 ] || for run in 1 2; do echo "--- run $run phone.log"; grep -E "mirror|layout|attach|embed|page |typed|swipe|screen " "$SHOTS/ios-mirror-here-run$run.log" | tail -30; done
echo "shots: $SHOTS"
finish
