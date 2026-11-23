#!/usr/bin/env bash
# iOS mode menu (wheel-0but.2, wheel-zxts.5.5): the top bar's title is the mode menu, with the space switcher under it, driven with real taps by the WheelPhoneUITests
# target (apple/iOS/UITests/ModeUITests.swift) against this script's own wheeld. The test writes
# `$SYNC/step-<name>` after each step and waits for `$SYNC/ack-<name>`; this script checks phone.log, then
# acknowledges. Shots and the bar's control rects go to
# $WHEEL_SHOTS (default $SCR/shots).
#
# Needs WHEEL_IOS_DERIVED_DATA: a derived data folder holding a `build-for-testing` of the WheelPhone
# scheme for the simulator (see project-verify § iOS title menu). Run with `env -u WHEEL_SPACE -u WHEEL_PANE`.
# WHEEL_MODE_BASE=1 makes the test write only mode-rects-launch.txt and mode-spaces-*.png, to measure the bar
# of a build from before the change.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/ios-ui-lib.sh"

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

mkdir -p "$SCR/wheel"
W="$(w space new "$SCR/wheel")"
P1="$(w space ls | jq -r --arg s "$W" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
P2="$(w pane split "$P1" right)"
w pane start "$P1" agent >/dev/null
w pane start "$P2" agent >/dev/null

if [ -n "${WHEEL_MODE_BASE:-}" ]; then
  ios_ui_start ModeUITests/testModeMenu WHEEL_MODE_BASE=1
  ios_ui_finish mode
fi
ios_ui_start ModeUITests/testModeMenu

if step agents; then
  check "phone.log restored Spaces at the first launch" plog "mode restored: spaces"
  check "phone.log records the pick" plog "mode menu: picked agents"
  check "phone.log records the change" plog "mode spaces -> agents"
  ack agents
fi
if step orchestrator; then
  check "phone.log records Orchestrator" plog "mode agents -> orchestrator"
  ack orchestrator
fi
if step spaces; then
  check "phone.log records Spaces" plog "mode orchestrator -> spaces"
  ack spaces
fi
if step keyboard; then
  check "a tap on the transcript hid the keyboard" plog "transcript tap: keyboard hidden"
  ack keyboard
fi
if step opened; then
  check "opening a pane from the eye's sheet in Agents showed Spaces" plog "mode agents -> spaces"
  ack opened
fi
if step relaunch; then
  check "phone.log restored Agents after the relaunch" plog "mode restored: agents"
  ack relaunch
fi
ios_ui_finish mode
