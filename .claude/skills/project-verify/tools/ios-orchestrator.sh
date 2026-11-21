#!/usr/bin/env bash
# iOS Orchestrator mode (wheel-eg4e.11): the Orchestrator space's carousel with its global orchestrator pinned,
# driven with real taps by the WheelPhoneUITests target (apple/iOS/UITests/OrchestratorUITests.swift) against this
# script's own wheeld, which holds one space with one agent pane and no Orchestrator space until the phone's Start.
# The test writes `$SYNC/step-<name>` after each step and waits for `$SYNC/ack-<name>`; this script checks wheeld
# and phone.log, then acknowledges. Shots go to $WHEEL_SHOTS (default $SCR/shots).
#
# Needs WHEEL_IOS_DERIVED_DATA: a derived data folder holding a `build-for-testing` of the WheelPhone
# scheme for the simulator (see project-verify § iOS title menu). Run with `env -u WHEEL_SPACE -u WHEEL_PANE`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/ios-ui-lib.sh"

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

mkdir -p "$SCR/wheel"
W="$(w space new "$SCR/wheel")"
P1="$(w space ls | jq -r --arg s "$W" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
w pane start "$P1" agent >/dev/null

ios_ui_start OrchestratorUITests/testOrchestratorMode

orchestrator_space() { w space ls | jq -r '.spaces[] | select(.orchestrator == true) | .id'; }
global_pane() { w space ls | jq -r '.spaces[] | select(.orchestrator == true) | [.root, (.popped // [])[], (.backgrounded // [])[] | .. | select(.node? == "pane" and .orchestrator? == "global")][0].id'; }

if step empty; then
  check "no Orchestrator space before Start" test -z "$(orchestrator_space)"
  check "phone.log records the mode" plog "mode spaces -> orchestrator"
  ack empty
fi
if step pinned; then
  O="$(orchestrator_space)"; G="$(global_pane)"
  check "Start made the Orchestrator space" test -n "$O"
  check "the Orchestrator space holds a global orchestrator pane" test -n "$G" -a "$G" != null
  check "phone.log records the start" plog "orchestrator start (req"
  check "phone.log records the reply" plog "orchestrator started in $G"
  check "the first open pinned that pane" plog "orchestrator space $O first open: pinned $G"
  ack pinned
fi
if step fullscreen; then ack fullscreen; fi
if step lists; then
  check "phone.log records Spaces" plog "mode orchestrator -> spaces"
  ack lists
fi
if step unpinned; then
  check "phone.log records the unpin" plog "unpin $(global_pane) in $(orchestrator_space)"
  check "phone.log restored Orchestrator mode after the relaunch" plog "mode restored: orchestrator"
  ack unpinned
fi
ios_ui_finish orchestrator
