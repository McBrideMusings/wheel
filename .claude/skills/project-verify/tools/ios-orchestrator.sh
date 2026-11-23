#!/usr/bin/env bash
# iOS Orchestrator mode (wheel-eg4e.11, wheel-eg4e.19.33): the Orchestrator space's carousel with the global
# orchestrator's chat as its first page, driven with real taps by the WheelPhoneUITests target (apple/iOS/UITests/OrchestratorUITests.swift) against this
# script's own wheeld, which holds one space with one agent pane and no Orchestrator space until the phone opens the mode.
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
# wheel-eg4e.20.3: a task file in the global orchestrator's folder, for the Tasks sheet.
mkdir -p "$INST/orchestrator/tasks"
printf '{"id":"tc1","asked":"Check the Tasks sheet","agents":["w0"],"status":"needs-review","created_ms":1,"done":false}\n' >"$INST/orchestrator/tasks/tc1.json"

ios_ui_start OrchestratorUITests/testOrchestratorMode

noplog() { ! plog "$1"; }
orchestrator_space() { w space ls | jq -r '.spaces[] | select(.orchestrator == true) | .id'; }
global_pane() { w space ls | jq -r '.spaces[] | select(.orchestrator == true) | [.root, (.popped // [])[], (.backgrounded // [])[] | .. | select(.node? == "pane" and .orchestrator? == "global")][0].id'; }

if step before; then
  check "no Orchestrator space before the mode opens" test -z "$(orchestrator_space)"
  ack before
fi
if step chat; then
  O="$(orchestrator_space)"; G="$(global_pane)"
  check "opening the mode made the Orchestrator space" test -n "$O"
  check "the Orchestrator space holds a global orchestrator pane" test -n "$G" -a "$G" != null
  check "phone.log records the mode" plog "mode spaces -> orchestrator"
  check "opening the mode started it, with no tap" plog "none runs"
  check "phone.log records the reply" plog "orchestrator started in $G"
  check "the chat is the first page" plog "orchestrator pages: $G"
  check "the title names the orchestrator" plog "title: Orchestrator"
  check "nothing pinned the orchestrator" noplog "pinned $G"
  ack chat
fi
if step chrome; then
  check "the handle's swipe up in Orchestrator mode opened no sheet" noplog "page handle: swipe up"
  check "no agents sheet opened" noplog "agents sheet: eye tapped"
  ack chrome
fi
if step tasks; then
  check "phone.log records the Tasks sheet" plog "tasks sheet: opened"
  check "the phone read the task file" plog "tasks: tc1 needs-review"
  check "Done wrote the person's done into the file" bash -c "jq -e '.done == true and .asked == \"Check the Tasks sheet\"' '$INST/orchestrator/tasks/tc1.json'"
  ack tasks
fi
if step keyboard; then
  # wheel-eg4e.19.41: with the keyboard up the chat, composer included, ends at the keyboard's top.
  kb_top() { grep -o 'keyboard up, top [0-9]*' "$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)/Documents/phone.log" | tail -1 | awk '{print $4}'; }
  on_keyboard() { local t b; t="$(kb_top)"; b="$(cat "$SYNC/chat-bottom" 2>/dev/null)"; echo "keyboard top $t, chat bottom $b"; [ -n "$t" ] && [ -n "$b" ] && [ $((t - b)) -ge -2 ] && [ $((t - b)) -le 2 ]; }
  check "the composer sits on the keyboard" on_keyboard
  ack keyboard
fi
if step model; then
  check "phone.log names the model pick" plog "set .* model to "
  check "phone.log names Clear chat" plog "clear chat "
  one_clear() { [ "$(grep -c 'clear chat ' "$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)/Documents/phone.log")" = 1 ]; }
  check "Cancel sent no /clear: one clear in phone.log" one_clear
  A="$(w status --json | jq -r --arg p "$G" '.. | objects | select(.id? == $p) | .agent // empty' | head -1)"
  M="$(cat "$SYNC/picked-model" 2>/dev/null)"
  took_model() { [ -n "$M" ] && w agent config "$A" 2>&1 | grep -i model | grep -q -- "$M"; }
  check "the agent took the picked model ($M)" took_model
  ack model
fi
if step fullscreen; then ack fullscreen; fi
if step spawn; then
  # The orchestrator opens a pane in its space, as its agent would with `wheel pane split`.
  N="$(w pane split "$G" right)"
  w pane rename "$N" notes >/dev/null
  check "the split made a pane in the Orchestrator space" test -n "$N"
  ack spawn
fi
if step page; then
  check "the new pane is a page after the chat" plog "orchestrator pages: $G, .*$N"
  check "the title named the new page" plog "title: notes"
  check "the floating button snapped to the left" plog "float: dragged to the left"
  check "a tap on the floating button showed the chat" plog "float: back to the chat"
  # The next step's reply names this pane and the agent pane in the other space.
  printf '%s %s' "$N" "$P1" >"$SYNC/link-panes"
  ack page
fi
if step link; then
  check "the link slid to the notes page" plog "orchestrator page $N"
  check "the link kept Orchestrator mode" noplog "mode orchestrator -> spaces"
  ack link
fi
if step scroll; then
  check "the arrow scrolled the transcript to its end" plog "scroll to bottom tapped"
  ack scroll
fi
if step lists; then
  check "phone.log records Spaces" plog "mode orchestrator -> spaces"
  ack lists
fi
if step relaunch; then
  check "phone.log restored Orchestrator mode on the notes page after the relaunch" plog "mode restored: orchestrator, orchestrator page $N"
  ack relaunch
fi
ios_ui_finish orchestrator
