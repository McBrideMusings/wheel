#!/usr/bin/env bash
# iOS earlier conversations (wheel-zxts.5.2): an agent pane draws the conversations it showed before its current one
# above it, dimmed, each behind a divider with its title and a Resume button. Claude Code fixture stores under
# CLAUDE_CONFIG_DIR hold C0, C1, C2 and C3 for the space's directory; the script resumes C0, C1, C2, then C3 in the space's one
# agent pane, so `visited` is [C0, C1, C2, C3], and then deletes C0's transcript. The WheelPhoneUITests target
# (apple/iOS/UITests/EarlierUITests.swift) checks three dividers in that order, the first one missing from the store
# with its Resume disabled, and taps C1's Resume; the script checks the pane is in C1. The test writes
# `$SYNC/step-<name>` after each step and waits for `$SYNC/ack-<name>`. Shots go to $WHEEL_SHOTS (default $SCR/shots).
#
# Needs WHEEL_IOS_DERIVED_DATA: a derived data folder holding a `build-for-testing` of the WheelPhone
# scheme for the simulator (see project-verify § iOS title menu). Run with `env -u WHEEL_SPACE -u WHEEL_PANE`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/ios-ui-lib.sh"

mkdir -p "$SCR/proj"
DIR="$(cd "$SCR/proj" && pwd -P)"
mangle() { printf '%s' "$1" | sed 's/[^A-Za-z0-9]/-/g'; }
export CLAUDE_CONFIG_DIR="$SCR/claude"
CP="$CLAUDE_CONFIG_DIR/projects/$(mangle "$DIR")"
mkdir -p "$CP"
claude_line() { printf '{"type":"user","uuid":"u-%s","timestamp":"2026-11-10T10:00:00.000Z","sessionId":"%s","cwd":"%s","message":{"role":"user","content":"%s"}}\n' "$1" "$1" "$DIR" "$2"; }
reply_line() { printf '{"type":"assistant","message":{"content":[{"type":"text","text":"%s"}]}}\n' "$1"; }
C0=00000000-0000-4000-8000-000000000000
C1=11111111-1111-4111-8111-111111111111
C2=22222222-2222-4222-8222-222222222222
C3=33333333-3333-4333-8333-333333333333
claude_line "$C0" "an old question" >"$CP/$C0.jsonl"
{ claude_line "$C1" "fix the flaky restore test"; reply_line "The restore test waited on a timer; it now waits on the save."; } >"$CP/$C1.jsonl"
{ claude_line "$C2" "add resume to the agent pane"; reply_line "Resume is in the header menu."; } >"$CP/$C2.jsonl"
claude_line "$C3" "why does deploy-client hang" >"$CP/$C3.jsonl"

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

S="$(w space new "$DIR")"
P="$(w space ls | jq -r --arg s "$S" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
w pane start "$P" agent >/dev/null
visited() { w space ls | jq -r --arg p "$P" '[.spaces[] | .root | .. | objects | select(.node? == "pane" and .id == $p) | .visited // []] | first // [] | join(",")'; }
for c in "$C0" "$C1" "$C2" "$C3"; do w agent resume "$P" "$c" >/dev/null || fail "resume $c"; done
eq "visited is C0, C1, C2, C3" "$(visited)" "$C0,$C1,$C2,$C3"
# C0 leaves the store, as a deleted transcript does.
rm "$CP/$C0.jsonl"
printf '%s %s %s %s' "$C0" "$C1" "$C2" "$C3" >"$SYNC/ids"

ios_ui_start EarlierUITests/testEarlierConversations

if step dividers; then
  check "the phone listed the earlier conversations" plog "pane $P earlier conversations: $C0,$C1,$C2"
  check "the phone read C1" plog "pane $P earlier $C1: [0-9]* events"
  check "the store lacks C0" plog "pane $P earlier $C0: not in the store"
  ack dividers
fi
if step resumed; then
  check "the phone sent Resume for C1" plog "pane $P resume $C1"
  in_c1() { [ "$(visited | tr ',' '\n' | tail -1)" = "$C1" ]; }
  check "the pane is in C1" poll 20 in_c1
  eq "visited ends with C1" "$(visited)" "$C0,$C2,$C3,$C1"
  ack resumed
fi
ios_ui_finish earlier
