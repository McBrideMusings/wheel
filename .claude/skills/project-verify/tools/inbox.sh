#!/usr/bin/env bash
# The orchestrator's inbox (wheel-eg4e.13), through the wheel command against the fake agent: a
# child agent that finishes while the orchestrator works on a turn adds an inbox item and sends
# the orchestrator nothing; once the orchestrator has been idle `--pull-after` seconds it is sent
# the item; two identical events from one agent are one item; `wheel inbox reply` delivers to the
# item's agent and acks it.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
trap cleanup EXIT

use_fake_agent
start_daemon --state-file "$SCR/state.json"
mkdir -p "$SCR/alpha" "$SCR/beta"
A="$(w space new "$SCR/alpha")"
B="$(w space new "$SCR/beta")"
# first_pane <space>: the id of the space's first pane.
first_pane() { w space ls | jq -r --arg s "$1" '[.spaces[] | select(.id == $s)][0] | [.. | objects | select(.id? and .kind?) | .id] | first'; }
PANE_A="$(first_pane "$A")"
PANE_B="$(first_pane "$B")"

V="$(w orchestrator start --space "$A" --pull-after 1)"
expect_out "^$A $V agent:orchestrator-space " "orchestrator start names its pane" w pane ls
w agent wait "$V" --timeout 30 >/dev/null 2>&1
check "an empty inbox lists nothing" bash -c "[ -z \"\$('$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' inbox list '$V')\" ]"

# Case 1: the orchestrator is mid-conversation (a 6 s turn) when a child agent finishes.
w agent send --timeout 0 "$V" "hold first 6000" >/dev/null
w pane start "$PANE_A" agent --message hello >/dev/null
w pane start "$PANE_B" agent --message hello >/dev/null
w agent wait "$PANE_A" --timeout 30 >/dev/null 2>&1
LIST=""
for _ in $(seq 1 50); do
  LIST="$(w inbox list "$V")"
  [ -n "$LIST" ] && break
  sleep 0.1
done
echo "inbox while the orchestrator works: $LIST"
CHILD="$(w inbox list "$V" --json | jq -r '.[0].agent')"
expect_out "^\[1\] $CHILD finished its turn$" "the finished child is one inbox item" printf %s "$LIST"
eq "the other space's agent is not in this inbox (no global orchestrator runs)" "$(printf '%s\n' "$LIST" | grep -c .)" "1"
MID="$(w agent read "$V" 2>&1)"
check "mid-turn the orchestrator was sent nothing" bash -c "! grep -q 'Inbox:' <<<\"\$1\"" _ "$MID"

# Case 2: idle for a second after its turn, the orchestrator is sent the item and mentions it.
SEEN=""
for _ in $(seq 1 120); do
  SEEN="$(w agent read "$V" 2>&1)"
  grep -q "echo: Inbox:" <<<"$SEEN" && break
  sleep 0.1
done
echo "$SEEN" | grep -A2 "echo: Inbox:" | head -3
check "the idle orchestrator was sent the item" grep -q "\[1\] $CHILD finished its turn" <<<"$SEEN"
expect_out "^\[1\] $CHILD finished its turn \(told\)$" "the item is marked told" w inbox list "$V"

# Case 3: the same event again is the same item.
w agent send "$CHILD" "hello again" --timeout 30 >/dev/null 2>&1
LIST=""
for _ in $(seq 1 50); do
  LIST="$(w inbox list "$V")"
  grep -q "2 times" <<<"$LIST" && break
  sleep 0.1
done
echo "after a second finished turn: $LIST"
expect_out "^\[1\] $CHILD finished its turn \(2 times\)" "two identical events are one item" printf %s "$LIST"
eq "still one item" "$(printf '%s\n' "$LIST" | grep -c .)" "1"

# Case 4: a reply reaches the item's agent and acks the item.
expect_out "^$CHILD turn [0-9]+$" "inbox reply names the agent and turn" w inbox reply "$V" 1 reply from the inbox
w agent wait "$CHILD" --timeout 30 >/dev/null 2>&1
check "the child agent got the reply" bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' agent read '$CHILD' 2>&1 | grep -q 'echo: reply from the inbox'"
expect_out "^\[1\] .*\(acked\)$" "the replied item is acked" w inbox list "$V" --all
check "an id that names no item is refused" bash -c "! '$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' inbox ack '$V' 999 2>/dev/null"

# A build or test command that fails is its own kind of item ("show tools" runs a failing `make build`).
w agent send "$CHILD" "show tools" --timeout 30 >/dev/null 2>&1
LIST=""
for _ in $(seq 1 50); do
  LIST="$(w inbox list "$V")"
  grep -q "build or test that failed" <<<"$LIST" && break
  sleep 0.1
done
echo "after a failed build: $LIST"
check "the failed build is an inbox item" grep -q "$CHILD ran a build or test that failed: make build" <<<"$LIST"
eq "its kind is build_failed" "$(w inbox list "$V" --json | jq -r '[.[] | select(.kind == "build_failed")] | length')" "1"
check "a turn limit of 0 is refused" bash -c "! '$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' orchestrator start --space '$A' --turn-limit 0 2>/dev/null"

w orchestrator stop "$V"
check "the inbox ends with its orchestrator" bash -c "! '$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' inbox list '$V' 2>/dev/null"
finish
