#!/usr/bin/env bash
# iOS queue tray and sheet (wheel-zxts.5.3, ADR-0054): in Orchestrator mode the header's tray counts what waits on the
# person in the orchestrator folder's queue (beads labelled needs-you), red with a P0, and its sheet lists the items
# under Needs you, Ready for review and Finished; a tap sends the item's message to the orchestrator's chat. Driven with
# real taps by the WheelPhoneUITests target (apple/iOS/UITests/QueueUITests.swift) against this script's own wheeld,
# which holds a space on the orchestrator folder with an agent pane and a backgrounded Queue pane (wheelhouse's
# `queue`, from this checkout's plugins/wheelhouse, linked into the folder as a project plugin). The queue holds a P0
# bug, a P2 question and a done item; after the tap the script closes the bug and the badge follows.
# The test writes `$SYNC/step-<name>` after each step and waits for `$SYNC/ack-<name>`; this script checks wheeld and
# phone.log, then acknowledges. Shots go to $WHEEL_SHOTS (default $SCR/shots).
#
# Needs WHEEL_IOS_DERIVED_DATA (a `build-for-testing` of the WheelPhone scheme for the simulator) and
# `cargo build` in this checkout (wheeld, wheel, the fake agent and wheelhouse). Run with `env -u WHEEL_SPACE -u WHEEL_PANE`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/ios-ui-lib.sh"

[ -x "$BIN/wheelhouse" ] || { echo "missing $BIN/wheelhouse: run cargo build in $REPO first" >&2; exit 2; }
PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
ODIR="$SCR/orchestrator"; mkdir -p "$ODIR/.wheel/plugins"
ln -s "$REPO/plugins/wheelhouse" "$ODIR/.wheel/plugins/wheelhouse"
printf '[remote]\nlisten = "127.0.0.1:%s"\n[orchestrator]\ndir = "%s"\n' "$RPORT" "$ODIR" >>"$INST/wheel.toml"

# The queue: its own beads database, as the orchestrator folder keeps one.
git -C "$ODIR" init -q
(cd "$ODIR" && bd init --non-interactive --stealth -p q >/dev/null 2>&1)
printf 'required = ["needs-you"]\n[labels.needs-you]\nabout = "An item in the orchestrator queue."\n' >"$ODIR/.beads/labels.toml"
item() { (cd "$ODIR" && bd create "$1" --labels needs-you --type "$2" --priority "$3" --metadata "$4" --silent); }
BUG="$(item "w3 failed" bug 0 '{"kind":"bug","agent":"w3","space":"logship"}')"
QUESTION="$(item "w4 asks: which port?" task 2 '{"kind":"question","agent":"w4","space":"wheel"}')"
DONE="$(item "w5 done: fixed the header" task 3 '{"kind":"done","agent":"w5","space":"wheel"}')"
mkdir -p "$SYNC"; printf '%s %s %s' "$BUG" "$QUESTION" "$DONE" >"$SYNC/ids"

start_daemon --state-file "$SCR/spaces.json"
O="$(w space new "$ODIR")"
G="$(w space ls | jq -r --arg s "$O" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
w pane start "$G" agent >/dev/null
Q="$(w pane split "$G" --background | awk '{print $1}')"
w pane kind "$Q" plugin --plugin wheelhouse --plugin-pane queue >/dev/null
check "the ids were written" test -n "$BUG" -a -n "$QUESTION" -a -n "$DONE"
check "the Queue pane runs wheelhouse's queue" sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' status --json | jq -e --arg p '$Q' '.. | objects | select(.id? == \$p) | select(.kind == \"plugin\" and .plugin == \"wheelhouse\" and .pane == \"queue\")' >/dev/null"

ios_ui_start QueueUITests/testQueueTray

if step tray; then
  check "phone.log reads the queue: 2, red" plog "queue $Q: 2 red"
  ack tray
fi
if step sheet; then
  check "phone.log records the tray's tap" plog "queue sheet: tray tapped, 2 waiting"
  ack sheet
fi
if step asked; then
  check "phone.log records the question sent to the chat" plog "queue: asked about $QUESTION (question)"
  A="$(w status --json | jq -r --arg p "$G" '.. | objects | select(.id? == $p) | .agent // empty' | head -1)"
  got_text() { w agent read "$A" 2>/dev/null | grep -q "Let's talk about $QUESTION: w4 asks: which port?"; }
  check "the orchestrator's agent got the item's message" got_text
  (cd "$ODIR" && bd close "$BUG" --reason "fixed" >/dev/null)
  ack asked
fi
if step closed; then
  check "phone.log follows the close: 1, gray" plog "queue $Q: 1 gray"
  ack closed
fi
ios_ui_finish queue
