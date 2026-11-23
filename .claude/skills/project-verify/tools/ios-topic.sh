#!/usr/bin/env bash
# iOS topic notes (wheel-zxts.5.4, ADR-0054): in Orchestrator mode the header's context line names the current topic
# (threads/current under the orchestrator folder, the title in threads/<id>.md's front matter) and a tap opens its notes
# rendered read-only; with no threads/current there is no line. The phone reads both with file_peek and again when the
# orchestrator's turn ends. Driven with real taps by the WheelPhoneUITests target (apple/iOS/UITests/TopicUITests.swift)
# against this script's own wheeld, which holds a space on the orchestrator folder with an agent pane (the fake agent).
# The script writes the topic files after the phone shows no line, then runs a turn so the phone reads again.
# The test writes `$SYNC/step-<name>` after each step and waits for `$SYNC/ack-<name>`; this script checks phone.log,
# then acknowledges. Shots go to $WHEEL_SHOTS (default $SCR/shots).
#
# Needs WHEEL_IOS_DERIVED_DATA (a `build-for-testing` of the WheelPhone scheme for the simulator) and
# `cargo build` in this checkout (wheeld, wheel and the fake agent). Run with `env -u WHEEL_SPACE -u WHEEL_PANE`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/ios-ui-lib.sh"

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
ODIR="$SCR/orchestrator"; mkdir -p "$ODIR"
printf '[remote]\nlisten = "127.0.0.1:%s"\n[orchestrator]\ndir = "%s"\n' "$RPORT" "$ODIR" >>"$INST/wheel.toml"

start_daemon --state-file "$SCR/spaces.json"
O="$(w space new "$ODIR")"
G="$(w space ls | jq -r --arg s "$O" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
w pane start "$G" agent >/dev/null

ios_ui_start TopicUITests/testTopicNotes

if step none; then
  check "phone.log names the folder" plog "orchestrator folder $ODIR"
  check "phone.log reads no topic yet" sh -c '! grep -q "topic wheel-abc" "$1"' _ "$PLOG"
  mkdir -p "$ODIR/threads"
  printf 'wheel-abc\n' >"$ODIR/threads/current"
  cat >"$ODIR/threads/wheel-abc.md" <<'EOF'
---
title: wheel · plugin hot reload
session: 5f0c2d4e
---
## Decisions

- Shared pane state lives in the backend feed

## Open

- Snapshot on quit or on every update?
EOF
  A="$(w status --json | jq -r --arg p "$G" '.. | objects | select(.id? == $p) | .agent // empty' | head -1)"
  check "the orchestrator's agent ran a turn" sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' agent send '$A' 'note it' >/dev/null"
  ack none
fi
if step line; then
  check "phone.log reads the topic after the turn" plog "topic wheel-abc: wheel · plugin hot reload, "
  ack line
fi
if step notes; then
  check "phone.log records the tap" plog "topic notes: opened wheel-abc"
  ack notes
fi
ios_ui_finish topic
