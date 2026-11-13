#!/usr/bin/env bash
# iOS command palette (wheel-zpa4.2.5): the search button's palette on the phone, driven with real
# taps by the WheelPhoneUITests target (apple/iOS/UITests/CommandPaletteUITests.swift) against this script's own wheeld. The test writes
# `$SYNC/step-<name>` after each step and waits for `$SYNC/ack-<name>`; this script checks wheeld
# and phone.log at each step, then acknowledges. Shots go to $WHEEL_SHOTS (default $SCR/shots).
#
# Needs WHEEL_IOS_DERIVED_DATA: a derived data folder holding a `build-for-testing` of the
# WheelPhone scheme for the simulator (see project-verify § iOS title menu). Run with
# `env -u WHEEL_SPACE -u WHEEL_PANE`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/ios-ui-lib.sh"

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

ios_ui_start CommandPaletteUITests/testPalette

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
ios_ui_finish palette
