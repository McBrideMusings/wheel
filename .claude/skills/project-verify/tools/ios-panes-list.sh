#!/usr/bin/env bash
# Phone panes button and list (wheel-v0aj): the button's orange dot, the list's pane rows and flat agent tiles
# sorted by need with the backgrounded ones behind a toggle, Allow and Deny answered from a tile, and a row or tile
# tap taking the phone to a popped-out, a backgrounded and another space's pane, driven with real taps by
# WheelPhoneUITests/PanesListUITests against this script's own wheeld. At each step the test writes
# `$SYNC/step-<name>`; this script changes and checks wheeld, then acknowledges.
#
# Needs WHEEL_IOS_DERIVED_DATA: a build-for-testing of the WheelPhone scheme for the simulator (see
# project-verify § iOS title menu). Run with `env -u WHEEL_SPACE -u WHEEL_PANE`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/ios-ui-lib.sh"

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

mkdir -p "$SCR/wheel" "$SCR/alpha"
S="$(w space new "$SCR/wheel")"
A="$(w space new "$SCR/alpha")"
first_pane() { w space ls | jq -r --arg s "$1" '[.spaces[] | select(.id==$s) | .root | .. | select(.node? == "pane")][0].id'; }
P1="$(first_pane "$S")"
P2="$(w pane split "$P1" right)"
P3="$(w pane split "$P2" right)"
P4="$(w pane split "$P1" down)"
P5="$(first_pane "$A")"
agent_of() { w pane start "$1" agent | jq -r '.agent'; }
N1="$(agent_of "$P1")"; N2="$(agent_of "$P2")"; N3="$(agent_of "$P3")"; N4="$(agent_of "$P4")"; N5="$(agent_of "$P5")"
w pane pop "$P3" >/dev/null
w pane background "$P4" >/dev/null
echo "panes replied=$P1/$N1 working=$P2/$N2 popped=$P3/$N3 backgrounded=$P4/$N4 alpha=$P5/$N5"

status_of() { w space ls | jq -r --arg p "$1" '.. | objects | select(.id? == $p and .node? == "pane") | .status.kind // "idle"'; }
wait_status() {
  local i
  for i in $(seq 1 60); do [ "$(status_of "$1")" = "$2" ] && break; sleep 0.25; done
  eq "$3" "$(status_of "$1")" "$2"
}

LONG="the session store compiles and every daemon test passes; next I wire the CLI to the new ops, then run the phone checks against a fresh wheeld, then write up what changed for the person"
w agent send "$N1" "$LONG" >/dev/null
w agent send "$N2" "work slowly 900" >/dev/null &
wait_status "$P2" working "$P2 works"
eq "both spaces are in one session" "$(w session ls --json 2>/dev/null | jq -r --arg s "$S" --arg a "$A" '[.[] | select((.spaces | index($s)) and (.spaces | index($a)))] | length')" 1
w pane focus "$P1" >/dev/null

phone_pane() { w client ls --json | jq -r '.[].windows[] | select(.window=="phone") | .pane // "none"'; }
expect_phone() {
  local i
  for i in $(seq 1 20); do [ "$(phone_pane)" = "$2" ] && break; sleep 0.25; done
  eq "$1" "$(phone_pane)" "$2"
}
transcript_has() { w agent read "$1" 2>/dev/null | grep -q -- "$2"; }

ios_ui_start PanesListUITests/testPanesList "WHEEL_AGENTS=$N1,$N2,$N3,$N4,$N5"
noplog() { ! plog "$1"; }

if step quiet; then
  ok "no agent needs the person: the panes button carries no orange dot (the UI test reads its value)"
  w agent send "$N4" "permission described" >/dev/null &
  wait_status "$P4" needs_you "the backgrounded agent $N4 needs the person"
  PERM4="$(w perm list | jq -r --arg a "$N4" '[.[] | select(.agent == $a)][0].id')"
  ack quiet
fi
lines_seen() { ls "$SYNC" | sed -n "s/^step-lines-$1-//p"; }
if step list; then
  eq "the replied tile's last message is two lines" "$(lines_seen replied)" 2
  check "phone.log has the button's tap" plog "panes button: open, 1 waiting"
  check "phone.log has the list opening on three panes" plog "panes list open: 3 panes"
  check "phone.log lists the panes, then the agents, needs you first" plog "panes list: panes .$P1, $P2, $P3., agents $N4=Needs you(bg), $N2=Working"
  check "phone.log has the backgrounded toggle" plog "panes list: show 1 backgrounded"
  ack list
fi
if step allowed; then
  check "Allow answered $N4's request ($PERM4)" plog "answering $PERM4: "
  for _ in $(seq 1 40); do transcript_has "$N4" "permission: allow" && break; sleep 0.25; done
  check "$N4's turn reports permission: allow" transcript_has "$N4" "permission: allow"
  eq "no request waits" "$(w perm list | jq 'length')" 0
  w agent send "$N1" "permission" >/dev/null &
  wait_status "$P1" needs_you "$N1 needs the person"
  ack allowed
fi
if step denied; then
  eq "a needs-you tile's last message is three lines" "$(lines_seen needs-you)" 3
  for _ in $(seq 1 40); do transcript_has "$N1" "permission: deny" && break; sleep 0.25; done
  check "Deny denied $N1's request" transcript_has "$N1" "permission: deny"
  ack denied
fi
if step open-backgrounded; then
  expect_phone "the backgrounded tile shows $P4 on the last page" "$P4"
  check "phone.log has the tile tap" plog "panes list: open $N4 in $S/$P4"
  ack open-backgrounded
fi
if step open-popped; then
  expect_phone "the popped-out row shows $P3" "$P3"
  check "phone.log has the row tap" plog "panes list: open $P3"
  ack open-popped
fi
if step open-tile; then
  expect_phone "$N1's tile shows $P1" "$P1"
  check "phone.log has the tile tap" plog "panes list: open $N1 in $S/$P1"
  ack open-tile
fi
if step closed; then
  check "phone.log has the second tap closing the list" plog "panes button: close"
  check "phone.log has the list closed" plog "panes list closed: 3 panes"
  ack closed
fi
if step open-alpha; then
  expect_phone "alpha's tile shows $P5" "$P5"
  eq "the phone shows space alpha" "$(w client ls --json | jq -r '.[].windows[] | select(.window=="phone") | .space')" "$A"
  check "phone.log has the Agents tab's tap" plog "agents tab: open $N5 in $A/$P5"
  w agent cancel "$N2" >/dev/null
  wait_status "$P2" done "$N2 stopped working (a cancelled turn reads done)"
  ack open-alpha
fi
if step all-idle; then
  for p in "$P1" "$P2" "$P3" "$P4" "$P5"; do w pane start "$p" terminal >/dev/null; done
  eq "no agent pane is left" "$(w pane ls | grep -c ' agent' || true)" 0
  ack all-idle
fi
if step empty; then
  ok "the list with no agents says so"
  ack empty
fi
ios_ui_finish panes
