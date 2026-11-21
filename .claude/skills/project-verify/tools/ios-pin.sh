#!/usr/bin/env bash
# iPhone pin sheet (wheel-eg4e.19.10): a pinned pane leaves the carousel for a bottom sheet at full, high, low or its header alone,
# driven with real taps and drags by the WheelPhoneUITests target (apple/iOS/UITests/PinUITests.swift) against this
# script's own wheeld: one space holding an agent, a terminal and a web pane. The test writes `$SYNC/step-<name>`
# after each step and waits for `$SYNC/ack-<name>`; this script checks phone.log and wheeld, then acknowledges.
# Shots of every position go to $WHEEL_SHOTS (default $SCR/shots).
#
# Needs WHEEL_IOS_DERIVED_DATA: a derived data folder holding a `build-for-testing` of the WheelPhone scheme for the
# simulator (see project-verify § iOS title menu). Run with `env -u WHEEL_SPACE -u WHEEL_PANE`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/ios-ui-lib.sh"

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

PROJ="$SCR/proj"
mkdir -p "$PROJ/site"
printf '<!doctype html><title>site</title><body>pinned page</body>\n' >"$PROJ/site/index.html"
printf '[artifacts.site]\npath = "site"\n' >"$PROJ/wheel.toml"
W="$(w space new "$PROJ")"
P1="$(w space ls | jq -r --arg s "$W" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
P2="$(w pane split "$P1" right)"
P3="$(w pane split "$P2" right)"
w pane start "$P1" agent >/dev/null
w pane start "$P2" terminal >/dev/null
w pane kind "$P3" web --artifact site >/dev/null
# What the Mac lays out: each node's place and kind, leaving out what runs in it (agent state, titles) and the focus.
tree() { w space ls | jq -c --arg s "$W" '.spaces[] | select(.id==$s) | [(.root, .popped[]?, .backgrounded[]?) | .. | objects | select(.node?) | {node, id, kind, site, axis, ratio}]'; }
BEFORE="$(tree)"

ios_ui_start PinUITests/testPinSheet WHEEL_PIN_PANES="$P1,$P2,$P3"

if step pinned; then
  check "phone.log records the pin at low" plog "pin $P1 in $W: pinned \[\"$P1\"\], sheet low"
  eq "wheeld's layout is unchanged by the pin (the Mac shows nothing different)" "$(tree)" "$BEFORE"
  ack pinned
fi
if step positions; then
  check "phone.log records low -> full" plog "sheet $W: low -> full"
  check "phone.log records full -> high" plog "sheet $W: full -> high"
  check "phone.log records high -> low" plog "sheet $W: high -> low"
  check "phone.log records low -> header" plog "sheet $W: low -> header"
  ack positions
fi
if step shown; then
  check "a drag up from the header brings it to low" plog "sheet $W: header -> low"
  ack shown
fi
# after <marker> <text>: phone.log holds <text> on a line after the last line holding <marker>.
after() { plog . && awk -v m="$1" -v t="$2" 'index($0, m) { seen = 1; found = 0; next } seen && index($0, t) { found = 1 } END { exit !found }' "$PLOG"; }
if step terminal; then
  check "phone.log records the terminal pinned" plog "pin $P2 in $W: pinned \[\"$P1\", \"$P2\"\]"
  TERM_ID="$(w space ls | jq -r --arg p "$P2" '.. | objects | select(.id? == $p) | .term')"
  check "the sheet takes the terminal once it is pinned" poll 10 after "pin $P2 in $W" "sheet terminal: $TERM_ID"
  ack terminal
fi
if step terminal-hidden; then
  check "the hidden sheet lets its terminal go" poll 10 after "sheet $W: low -> header" "sheet terminal: none"
  check "the hidden sheet's terminal is detached" poll 10 after "sheet $W: low -> header" "detached from $TERM_ID"
  ack terminal-hidden
fi
if step terminal-shown; then
  check "the sheet brought back attaches its terminal again" poll 10 after "sheet $W: header -> low" "attaching to $TERM_ID"
  ack terminal-shown
fi
if step web; then
  check "phone.log records the web pane pinned" plog "pin $P3 in $W: pinned \[\"$P1\", \"$P2\", \"$P3\"\]"
  eq "wheeld's layout is still unchanged with every pane pinned" "$(tree)" "$BEFORE"
  ack web
fi
if step switched; then
  check "the header menu switched the sheet to the agent" plog "sheet $W: shows $P1"
  ack switched
fi
if step unpinned; then
  check "phone.log records the unpin" plog "unpin $P1 in $W: pinned \[\"$P2\", \"$P3\"\]"
  ack unpinned
fi
if step relaunch; then
  check "the pins came back after the relaunch" plog "pins restored: $W=$P2,$P3"
  w pane close "$P3" >/dev/null
  check "a pinned pane that closed left the pins" poll 10 plog "a pinned pane closed"
  ack relaunch
fi
if step none; then
  check "phone.log records the last unpin" plog "unpin $P2 in $W: pinned \[\]"
  ack none
fi
ios_ui_finish pin
