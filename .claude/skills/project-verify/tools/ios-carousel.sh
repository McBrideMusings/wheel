#!/usr/bin/env bash
# Phone carousel pages (wheel-f6y5.5): grid panes in reading order, then the popped window's panes
# with ring dots, then the last page listing only the backgrounded panes, driven with real swipes and
# taps by WheelPhoneUITests/CarouselUITests against this script's own wheeld. At each step the test
# writes `$SYNC/step-<name>`; this script checks wheeld and phone.log, then acknowledges.
#
# Needs WHEEL_IOS_DERIVED_DATA: a build-for-testing of the WheelPhone scheme for the simulator (see
# project-verify § iOS title menu). Run with `env -u WHEEL_SPACE -u WHEEL_PANE`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/ios-ui-lib.sh"

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

mkdir -p "$SCR/wheel"
S="$(w space new "$SCR/wheel")"
P1="$(w space ls | jq -r --arg s "$S" '[.spaces[] | select(.id==$s) | .root | .. | select(.node? == "pane")][0].id')"
P2="$(w pane split "$P1" right)"
P3="$(w pane split "$P2" right)"
P5="$(w pane split "$P1" down)"
P6="$(w pane split "$P2" down)"
for p in "$P1" "$P2" "$P3" "$P5"; do w pane start "$p" agent >/dev/null; done
w pane start "$P6" terminal >/dev/null
w pane pop "$P3" >/dev/null
P4="$(w pane split "$P3" down)"
w pane start "$P4" agent >/dev/null
w pane background "$P5" >/dev/null
w pane background "$P6" >/dev/null

space_json() { w space ls | jq -c --arg s "$S" '.spaces[] | select(.id==$s)'; }
leaves() { jq -c '[.. | select(.node? == "pane") | .id]'; }
eq "the fixture's grid is $P1, $P2" "$(space_json | jq '.root' | leaves)" "[\"$P1\",\"$P2\"]"
eq "one popped window holds $P3 over $P4" "$(space_json | jq -c '[.popped[] | [.. | select(.node? == "pane") | .id]]')" "[[\"$P3\",\"$P4\"]]"
eq "the tray holds $P5, $P6" "$(space_json | jq -c '[.backgrounded[] | .. | select(.node? == "pane") | .id]')" "[\"$P5\",\"$P6\"]"
w pane focus "$P1" >/dev/null

phone_pane() { w client ls --json | jq -r '.[].windows[] | select(.window=="phone") | .pane // "none"'; }
expect_phone() {
  local i
  for i in $(seq 1 20); do [ "$(phone_pane)" = "$2" ] && break; sleep 0.25; done
  eq "$1" "$(phone_pane)" "$2"
}

ios_ui_start CarouselUITests/testCarousel "WHEEL_TRAY=$P5,$P6"

n=0
for p in "$P1" "$P2" "$P3" "$P4"; do
  n=$((n + 1))
  if step "page-$n"; then
    expect_phone "page $n shows $p" "$p"
    ack "page-$n"
  fi
done
if step tray-list; then
  ok "the last page lists $P5 and $P6 only, with no status dot (the UI test asserts the rows)"
  ack tray-list
fi
if step tray-open; then
  expect_phone "a row tap shows $P5 in place" "$P5"
  check "phone.log has the tap" plog "tray: open $P5"
  ack tray-open
fi
if step tray-back; then
  check "phone.log has the Back tap" plog "tray: back"
  w agent send "$P5" "permission described" >/dev/null &
  for _ in $(seq 1 40); do [ "$(w space ls | jq -r --arg p "$P5" '.. | objects | select(.id? == $p and .node? == "pane") | .status.kind // ""')" = needs_you ] && break; sleep 0.25; done
  eq "$P5 needs the person" "$(w space ls | jq -r --arg p "$P5" '.. | objects | select(.id? == $p and .node? == "pane") | .status.kind // ""')" needs_you
  ack tray-back
fi
if step needs-you; then
  ok "the tray's mark carries the needs-you badge (the UI test reads tray!)"
  ack needs-you
fi
if step reopened; then
  eq "$P5 back in the grid" "$(w pane background "$P5")" grid
  ack reopened
fi
if step left-tray; then
  ok "the open pane leaving the tray gave way to the list of $P6"
  w pane background "$P6" >/dev/null
  ack left-tray
fi
if step no-tray; then
  eq "the tray is empty" "$(space_json | jq -c '.backgrounded | length')" 0
  ack no-tray
fi
ios_ui_finish carousel
