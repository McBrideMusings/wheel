#!/usr/bin/env bash
# iPhone pane menu push switch (wheel-zxts.8): the ⋯ menu's Turn Notifications Off / On sets the pane's `quiet` in
# wheeld (`pane_push`), driven with real taps by the WheelPhoneUITests target (apple/iOS/UITests/PushMenuUITests.swift)
# against this script's own wheeld: one space holding one pane. Shots of the open menu go to $WHEEL_SHOTS.
#
# Needs WHEEL_IOS_DERIVED_DATA: a derived data folder holding a `build-for-testing` of the WheelPhone scheme for the
# simulator (see project-verify § iOS title menu). Run with `env -u WHEEL_SPACE -u WHEEL_PANE`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/ios-ui-lib.sh"

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

mkdir -p "$SCR/proj"
W="$(w space new "$SCR/proj")"
P1="$(w space ls | jq -r --arg s "$W" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
quiet() { w space ls | jq -r --arg p "$P1" '[.. | objects | select(.id? == $p)][0].quiet // false'; }
is_quiet() { [ "$(quiet)" = "$1" ]; }

ios_ui_start PushMenuUITests/testPushToggle

if step off; then
  check "phone.log records the push off" plog "pane op: {\"on\":false,\"op\":\"pane_push\",\"pane\":\"$P1\""
  check "Turn Notifications Off made the pane quiet in wheeld" poll 10 is_quiet true
  ack off
fi
if step on; then
  check "phone.log records the push on" plog "pane op: {\"on\":true,\"op\":\"pane_push\",\"pane\":\"$P1\""
  check "Turn Notifications On let the pane push again" poll 10 is_quiet false
  ack on
fi
ios_ui_finish push
