#!/usr/bin/env bash
# iPhone Thumb navigation (epic wheel-l1n4): the top-left space name with its needs-you count, the
# Thumb sheet (flat Spaces list, session bar, Agents tab with inline permission answers), driven
# with real taps and swipes by the WheelPhoneUITests target (apple/iOS/UITests/ThumbUITests.swift)
# against this script's own wheeld. The test writes `$SYNC/step-<name>` after each step and waits
# for `$SYNC/ack-<name>`; this script checks wheeld at each step, then acknowledges. Shots go to
# $WHEEL_SHOTS (default $SCR/shots), light and dark.
#
# The fixture: session Main holds wheel (two agents working, one idle), alpha (two agents that
# will wait on a permission, one idle) and beta (one waiting, one idle); session Mobile holds app;
# session Games holds chess and twelve fillers.
#
# Needs WHEEL_IOS_DERIVED_DATA: a build-for-testing of the WheelPhone scheme (verify.md § iOS
# title menu). Run with `env -u WHEEL_SPACE -u WHEEL_PANE`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SIM=~/.claude/tools/sim
BUNDLE=com.piercemakes.wheel.phone
UDID=""
TPID=""
ios_cleanup() {
  [ -n "$TPID" ] && kill "$TPID" 2>/dev/null
  [ -n "$UDID" ] && "$SIM" release >/dev/null 2>&1
  cleanup
}
trap ios_cleanup EXIT

DD="${WHEEL_IOS_DERIVED_DATA:-}"
[ -d "$DD/Build/Products" ] || { echo "set WHEEL_IOS_DERIVED_DATA to a build-for-testing of WheelPhone" >&2; exit 2; }
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
SYNC="$SCR/sync"
mkdir -p "$SHOTS" "$SYNC"

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

first_pane() { w space ls | jq -r --arg s "$1" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id'; }
for d in wheel alpha beta app chess; do mkdir -p "$SCR/$d"; done
W="$(w space new "$SCR/wheel")"
SESSION="$(w session ls --json | jq -r '.[0].id')"
w session rename "$SESSION" Main >/dev/null
A="$(w space new "$SCR/alpha" --session Main)"
B="$(w space new "$SCR/beta" --session Main)"
w session new Mobile >/dev/null
w space new "$SCR/app" --session Mobile >/dev/null
w session new Games >/dev/null
w space new "$SCR/chess" --session Games >/dev/null
for i in $(seq 1 12); do mkdir -p "$SCR/fill$i"; w space new "$SCR/fill$i" --session Games >/dev/null; done

WP1="$(first_pane "$W")"; WP2="$(w pane split "$WP1" right)"; WP3="$(w pane split "$WP2" right)"
AP1="$(first_pane "$A")"; AP2="$(w pane split "$AP1" right)"; AP3="$(w pane split "$AP2" right)"
BP1="$(first_pane "$B")"; BP2="$(w pane split "$BP1" right)"
for p in $WP1 $WP2 $WP3 $AP1 $AP2 $AP3 $BP1 $BP2; do w pane start "$p" agent >/dev/null; done
bg w agent send "$WP1" "work slowly 600"
bg w agent send "$WP2" "work slowly 600"
count_status() { w space ls | jq --arg k "$1" '[.spaces[].root | .. | select(.status?.kind == $k)] | length'; }
for _ in $(seq 1 60); do [ "$(count_status working)" = 2 ] && break; sleep 0.5; done
eq "two agents are working" "$(count_status working)" 2
eq "no agent waits yet" "$(count_status needs_you)" 0

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
CODE="$(w remote pair --address 127.0.0.1)"
XRUN="$(ls "$DD"/Build/Products/*.xctestrun | head -1)"
TEST_RUNNER_WHEEL_PAIR_JSON="$CODE" TEST_RUNNER_WHEEL_SYNC_DIR="$SYNC" TEST_RUNNER_WHEEL_SHOTS="$SHOTS" \
  xcodebuild test-without-building -xctestrun "$XRUN" -destination "id=$UDID" \
  -only-testing:WheelPhoneUITests/ThumbUITests >"$SCR/xcodebuild.log" 2>&1 &
TPID=$!

step() {
  local i
  for i in $(seq 1 1200); do
    [ -f "$SYNC/step-$1" ] && return 0
    { kill -0 "$TPID" 2>/dev/null && ! grep -qE "Test Suite 'Selected tests' (passed|failed)" "$SCR/xcodebuild.log"; } || { fail "the UI test reached $1" "$(grep -E 'error:|XCTAssert' "$SCR/xcodebuild.log" | head -5)"; return 1; }
    sleep 0.25
  done
  fail "the UI test reached $1 within 5 minutes"
  return 1
}
ack() { touch "$SYNC/ack-$1"; }

if step quiet; then
  ok "with nobody waiting the top-left count slot is blank (the UI test asserts it and measures the name)"
  for p in $AP1 $AP2 $BP1; do bg w agent send "$p" "permission described"; done
  for _ in $(seq 1 60); do [ "$(count_status needs_you)" = 3 ] && break; sleep 0.5; done
  eq "three agents wait on a permission" "$(count_status needs_you)" 3
  ack quiet
fi
if step waiting; then
  ok "with three waiting the count reads 3 and the name stays where it was (the UI test asserts both)"
  ack waiting
fi

if step sheet; then
  ok "the sheet listed Main's spaces flat in session order, kept its controls' rects through Main, Mobile and Games, swiped along the session bar and filtered by search (the UI test asserts each)"
  eq "client ls has the phone on beta in Main" \
    "$(w client ls --json | jq -r '.[].windows[] | select(.window=="phone") | "\(.session) \(.space)"')" "$SESSION $B"
  ack sheet
fi

for _ in $(seq 1 120); do grep -qE "Test Suite 'Selected tests' (passed|failed)" "$SCR/xcodebuild.log" && break; sleep 0.5; done
if grep -q "Test Suite 'Selected tests' passed" "$SCR/xcodebuild.log"; then TEST_EXIT=0; else TEST_EXIT=1; fi
kill "$TPID" 2>/dev/null
TPID=""
eq "the UI test passed" "$TEST_EXIT" 0
[ "$TEST_EXIT" = 0 ] || grep -E "error:|XCTAssert" "$SCR/xcodebuild.log" | head -10
for f in "$SHOTS"/thumb-*.png; do echo "shot $f"; done
finish
