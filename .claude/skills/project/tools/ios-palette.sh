#!/usr/bin/env bash
# iOS command palette (wheel-zpa4.2.5): the search button's palette on the phone, driven with real
# taps by the WheelPhoneUITests target (apple/iOS/UITests/CommandPaletteUITests.swift) against this script's own wheeld. The test writes
# `$SYNC/step-<name>` after each step and waits for `$SYNC/ack-<name>`; this script checks wheeld
# and phone.log at each step, then acknowledges. Shots go to $WHEEL_SHOTS (default $SCR/shots).
#
# Needs WHEEL_IOS_DERIVED_DATA: a derived data folder holding a `build-for-testing` of the
# WheelPhone scheme for the simulator (see verify.md § iOS title menu). Run with
# `env -u WHEEL_SPACE -u WHEEL_PANE`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SIM=~/.claude/tools/sim
BUNDLE=com.piercemakes.wheel.phone
UDID=""
TPID=""
ios_cleanup() {
  [ -n "$DPID" ] && kill -CONT "$DPID" 2>/dev/null
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
pane_count() { w space ls | jq --arg s "$1" '[.spaces[] | select(.id==$s) | .root | .. | select(.node? == "pane")] | length'; }
session_of() { w session ls --json | jq -r --arg s "$1" '.[] | select(.spaces | index($s)) | .name'; }
space_named() { w space ls | jq -r --arg n "$1" '.spaces[] | select(.name==$n) | .id'; }

for d in wheel alpha chess; do mkdir -p "$SCR/$d"; done
W="$(w space new "$SCR/wheel")"
SESSION="$(w session ls --json | jq -r '.[0].id')"
w session rename "$SESSION" wheel >/dev/null
A="$(w space new "$SCR/alpha" --session wheel)"
w session new games >/dev/null
C="$(w space new "$SCR/chess" --session games)"
w artifact new dash --pane --space "$W" >/dev/null
P1="$(first_pane "$W")"
P2="$(w pane split "$P1" right)"
w pane start "$P1" agent >/dev/null
w pane start "$P2" agent >/dev/null
bg w agent send "$P1" "work slowly 600"
for _ in $(seq 1 60); do
  [ "$(w space ls | jq --arg s "$W" '[.spaces[] | select(.id==$s) | .root | .. | select(.status?.kind == "working")] | length')" = 1 ] && break
  sleep 0.5
done

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
CODE="$(w remote pair --address 127.0.0.1)"
XRUN="$(ls "$DD"/Build/Products/*.xctestrun | head -1)"
TEST_RUNNER_WHEEL_PAIR_JSON="$CODE" TEST_RUNNER_WHEEL_SYNC_DIR="$SYNC" TEST_RUNNER_WHEEL_SHOTS="$SHOTS" \
  xcodebuild test-without-building -xctestrun "$XRUN" -destination "id=$UDID" \
  -only-testing:WheelPhoneUITests/CommandPaletteUITests/testPalette >"$SCR/xcodebuild.log" 2>&1 &
TPID=$!

PLOG=""
plog() {
  if [ -z "$PLOG" ]; then
    local data; data="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data 2>/dev/null)" || return 1
    PLOG="$data/Documents/phone.log"
  fi
  grep -q -- "$1" "$PLOG" 2>/dev/null
}
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
pane_count() { w space ls | jq --arg s "$1" '[.spaces[] | select(.id==$s) | .root | .. | select(.node? == "pane")] | length'; }
space_named() { w space ls | jq -r --arg n "$1" '.spaces[] | select(.name==$n) | .id'; }
shown() { w client ls --json | jq -r '.[].windows[] | select(.window=="phone") | "\(.session) \(.space)"'; }

if step chess; then
  check "phone.log lists the palette's rows" plog "palette opened: space:$W"
  check "choosing the chess row ran it" plog "palette run space:$C"
  eq "client ls has the phone on chess" "$(shown)" "$(w session ls --json | jq -r --arg c "$C" '.[] | select(.spaces | index($c)) | .id') $C"
  ack chess
fi
if step pane; then
  check "choosing the pane row ran it" plog "palette run pane:$P1"
  eq "client ls has the phone on wheel" "$(shown)" "$SESSION $W"
  ack pane
fi
if step artifact; then
  for _ in $(seq 1 20); do [ "$(pane_count "$W")" = 3 ] && break; sleep 0.25; done
  eq "the artifact row opened a pane: wheel holds 3" "$(pane_count "$W")" 3
  ack artifact
fi
if step new-agent; then
  for _ in $(seq 1 20); do [ "$(pane_count "$W")" = 4 ] && break; sleep 0.25; done
  eq "the new agent row made a pane: wheel holds 4" "$(pane_count "$W")" 4
  eq "the new pane runs an agent" "$(w space ls | jq --arg s "$W" '[.spaces[] | select(.id==$s) | .root | .. | select(.node? == "pane" and .kind == "agent")] | length')" 3
  ack new-agent
fi
if step renamed; then
  for _ in $(seq 1 20); do [ "$(space_named wheel2)" = "$W" ] && break; sleep 0.25; done
  eq "Rename space…: wheel space ls names $W wheel2" "$(space_named wheel2)" "$W"
  ack renamed
fi
if step kept; then
  eq "Close space, then Cancel, keeps wheel2" "$(space_named wheel2)" "$W"
  check "phone.log shows the palette asked first" plog "palette run action:close-space"
  ack kept
fi
for _ in $(seq 1 120); do grep -qE "Test Suite 'Selected tests' (passed|failed)" "$SCR/xcodebuild.log" && break; sleep 0.5; done
if grep -q "Test Suite 'Selected tests' passed" "$SCR/xcodebuild.log"; then TEST_EXIT=0; else TEST_EXIT=1; fi
kill "$TPID" 2>/dev/null
TPID=""
eq "the UI test passed" "$TEST_EXIT" 0
[ "$TEST_EXIT" = 0 ] || grep -E "error:|XCTAssert" "$SCR/xcodebuild.log" | head -10
for f in "$SHOTS"/palette-*.png; do echo "shot $f"; done
finish
