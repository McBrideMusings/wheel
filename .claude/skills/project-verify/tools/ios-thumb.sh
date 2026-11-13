#!/usr/bin/env bash
# iPhone Thumb navigation (epic wheel-l1n4): the top-left space name with its needs-you count, the
# Thumb sheet (flat Spaces list, session bar, Agents tab with inline permission answers), driven
# with real taps and swipes by the WheelPhoneUITests target (apple/iOS/UITests/ThumbUITests.swift)
# against this script's own wheeld. The test writes `$SYNC/step-<name>` after each step and waits
# for `$SYNC/ack-<name>`; this script checks wheeld at each step, then acknowledges. Shots go to
# $WHEEL_SHOTS (default $SCR/shots), light and dark.
#
# The fixture: session Main holds wheel (two agents working, two idle), alpha (two agents that
# will wait on a permission, one idle) and beta (one waiting, one idle); session Mobile holds app;
# session Games holds chess and twelve fillers.
#
# Needs WHEEL_IOS_DERIVED_DATA: a build-for-testing of the WheelPhone scheme (project-verify § iOS
# title menu). Run with `env -u WHEEL_SPACE -u WHEEL_PANE`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/ios-ui-lib.sh"

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

WP1="$(first_pane "$W")"; WP2="$(w pane split "$WP1" right)"; WP3="$(w pane split "$WP2" right)"; WP4="$(w pane split "$WP3" right)"
AP1="$(first_pane "$A")"; AP2="$(w pane split "$AP1" right)"; AP3="$(w pane split "$AP2" right)"
BP1="$(first_pane "$B")"; BP2="$(w pane split "$BP1" right)"
for p in $WP1 $WP2 $WP3 $WP4 $AP1 $AP2 $AP3 $BP1 $BP2; do w pane start "$p" agent >/dev/null; done
bg w agent send "$WP1" "work slowly 600"
bg w agent send "$WP2" "work slowly 600"
count_status() { w space ls | jq --arg k "$1" '[.spaces[].root | .. | select(.status?.kind == $k)] | length'; }
for _ in $(seq 1 60); do [ "$(count_status working)" = 2 ] && break; sleep 0.5; done
eq "two agents are working" "$(count_status working)" 2
eq "no agent waits yet" "$(count_status needs_you)" 0

ios_ui_start ThumbUITests


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
if step allowed; then
  for _ in $(seq 1 40); do [ "$(count_status needs_you)" = 2 ] && break; sleep 0.25; done
  eq "Allow on the Agents tab answered one request: two agents still wait" "$(count_status needs_you)" 2
  eq "wheeld holds two waiting requests" "$(w perm list | jq length)" 2
  ack allowed
fi
if step opened; then
  eq "a tap on alpha's agent row took the phone to alpha in Main" \
    "$(w client ls --json | jq -r '.[].windows[] | select(.window=="phone") | "\(.session) \(.space)"')" "$SESSION $A"
  ack opened
fi
if step handle; then
  check "phone.log has a short swipe opening the sheet on Spaces" plog 'page handle: swipe up [0-9]* pt, sheet on spaces'
  check "phone.log has a long swipe opening the sheet on Agents" plog 'page handle: swipe up [0-9]* pt, sheet on agents'
  ack handle
fi

ios_ui_finish thumb
