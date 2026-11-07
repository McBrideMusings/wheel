#!/usr/bin/env bash
# iOS title menu (wheel-zpa4.1.6): sessions, spaces, instant switching and space actions on the
# phone, driven with real taps and long presses by the WheelPhoneUITests target
# (apple/iOS/UITests/TitleMenuUITests.swift) against this script's own wheeld. The test writes
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

for d in wheel alpha beta chess; do mkdir -p "$SCR/$d"; done
W="$(w space new "$SCR/wheel")"
SESSION="$(w session ls --json | jq -r '.[0].id')"
w session rename "$SESSION" wheel >/dev/null
A="$(w space new "$SCR/alpha" --session wheel)"
B="$(w space new "$SCR/beta" --session wheel)"
w session new games >/dev/null
C="$(w space new "$SCR/chess" --session games)"
for i in $(seq 1 12); do mkdir -p "$SCR/fill$i"; w space new "$SCR/fill$i" --session games >/dev/null; done
P1="$(first_pane "$W")"
P2="$(w pane split "$P1" right)"
w pane start "$P1" agent >/dev/null
w pane start "$P2" agent >/dev/null
bg w agent send "$P1" "work slowly 600"
bg w agent send "$P2" "work slowly 600"
check "the fixture: wheel holds wheel, alpha and beta; games holds chess and twelve fillers" eq "sessions" \
  "$(w session ls --json | jq -c '[.[] | {name, n: (.spaces | length)}]')" '[{"name":"wheel","n":3},{"name":"games","n":13}]'
for _ in $(seq 1 60); do
  [ "$(w space ls | jq --arg s "$W" '[.spaces[] | select(.id==$s) | .root | .. | select(.status?.kind == "working")] | length')" = 2 ] && break
  sleep 0.5
done
eq "both agents in wheel are working" "$(w space ls | jq --arg s "$W" '[.spaces[] | select(.id==$s) | .root | .. | select(.status?.kind == "working")] | length')" 2

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
CODE="$(w remote pair --address 127.0.0.1)"
XRUN="$(ls "$DD"/Build/Products/*.xctestrun | head -1)"
TEST_RUNNER_WHEEL_PAIR_JSON="$CODE" TEST_RUNNER_WHEEL_SYNC_DIR="$SYNC" TEST_RUNNER_WHEEL_SHOTS="$SHOTS" \
  xcodebuild test-without-building -xctestrun "$XRUN" -destination "id=$UDID" \
  -only-testing:WheelPhoneUITests/TitleMenuUITests/testTitleMenu >"$SCR/xcodebuild.log" 2>&1 &
TPID=$!

PLOG=""
plog() {
  if [ -z "$PLOG" ]; then
    local data; data="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data 2>/dev/null)" || return 1
    PLOG="$data/Documents/phone.log"
  fi
  grep -q -- "$1" "$PLOG" 2>/dev/null
}
# step <name>: wait for the test to reach it; ack <name> lets the test go on.
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
requests() { w status --json | jq -c '.requests | with_entries(select(.key | startswith("device ")))'; }

if step stall; then
  ok "the menu lists wheel, alpha and beta and not chess (the UI test asserts each row)"
  check "phone.log lists the shown session's spaces only" plog "title menu: session $SESSION spaces \[$W, $A, $B\]"
  BEFORE="$(requests)"
  kill -STOP "$DPID"
  ack stall
fi
if step switched; then
  ok "with wheeld stopped, tapping alpha drew alpha in the title within 3 s"
  kill -CONT "$DPID"
  sleep 1
  AFTER="$(requests)"
  eq "the switch sent wheeld nothing but the view report" \
    "$(jq -cn --argjson a "$BEFORE" --argjson b "$AFTER" '[$b | to_entries[] | select(.value != ($a[.key] // 0)) | .key] | map(select(. != "device log")) | sort')" '["device view"]'
  eq "client ls has the phone on alpha in session wheel" \
    "$(w client ls --json | jq -r '.[].windows[] | select(.window=="phone") | "\(.session) \(.space)"')" "$SESSION $A"
  ack switched
fi
if step second-pane; then
  eq "the phone shows wheel's second pane before the relaunch" \
    "$(w client ls --json | jq -r '.[].windows[] | select(.window=="phone") | "\(.space) \(.pane)"')" "$W $P2"
  ack second-pane
fi
if step relaunched; then
  check "the relaunch restored session, space and pane" plog "restored last view session=$SESSION space=$W pane=$P2: took space, showing $SESSION/$W/$P2"
  eq "client ls has the relaunched phone on wheel's second pane" \
    "$(w client ls --json | jq -r '.[].windows[] | select(.window=="phone") | "\(.session) \(.space) \(.pane)"')" "$SESSION $W $P2"
  ack relaunched
fi
if step renamed; then
  for _ in $(seq 1 20); do [ "$(space_named beta2)" = "$B" ] && break; sleep 0.25; done
  eq "Rename: wheel space ls names $B beta2" "$(space_named beta2)" "$B"
  ack renamed
fi
if step moved; then
  for _ in $(seq 1 20); do [ "$(session_of "$B")" = games ] && break; sleep 0.25; done
  eq "Move to session: wheel session ls puts beta2 in games" "$(session_of "$B")" games
  ack moved
fi
if step new-pane; then
  for _ in $(seq 1 20); do [ "$(pane_count "$W")" = 3 ] && break; sleep 0.25; done
  eq "New pane: wheel holds 3 panes" "$(pane_count "$W")" 3
  ack new-pane
fi
if step closed-alpha; then
  for _ in $(seq 1 20); do [ -z "$(space_named alpha)" ] && break; sleep 0.25; done
  eq "Close on a space with no working agent: alpha is gone at once" "$(space_named alpha)" ""
  check "phone.log says alpha closed without asking" plog "close $A: no agent working, closed at once"
  ack closed-alpha
fi
if step kept-wheel; then
  ok "Close on wheel showed \"Close wheel? 2 agents are working. Closing ends them and every terminal in it.\" (the UI test asserts the text)"
  eq "Cancel keeps wheel" "$(space_named wheel)" "$W"
  ack kept-wheel
fi
if step closed-wheel; then
  for _ in $(seq 1 20); do [ -z "$(space_named wheel)" ] && break; sleep 0.25; done
  eq "Close space in the dialog closes wheel" "$(space_named wheel)" ""
  eq "session wheel is left empty and games keeps chess, beta2 and the twelve fillers" \
    "$(w session ls --json | jq -c '[.[] | {name, n: (.spaces | length)}]')" '[{"name":"wheel","n":0},{"name":"games","n":14}]'
  ack closed-wheel
fi
# xcodebuild can stay up after a failed test; the suite's last line is the end of the run.
for _ in $(seq 1 120); do grep -qE "Test Suite 'Selected tests' (passed|failed)" "$SCR/xcodebuild.log" && break; sleep 0.5; done
if grep -q "Test Suite 'Selected tests' passed" "$SCR/xcodebuild.log"; then TEST_EXIT=0; else TEST_EXIT=1; fi
kill "$TPID" 2>/dev/null
TPID=""
eq "the UI test passed" "$TEST_EXIT" 0
[ "$TEST_EXIT" = 0 ] || grep -E "error:|XCTAssert" "$SCR/xcodebuild.log" | head -10
for f in "$SHOTS"/title-menu-*.png; do echo "shot $f"; done
finish
