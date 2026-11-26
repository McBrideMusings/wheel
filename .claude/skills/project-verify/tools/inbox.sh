#!/usr/bin/env bash
# The orchestrator's inbox (wheel-eg4e.20.1, ADR-0054), through the wheel command against the fake
# agent: each agent event is one JSON file in the orchestrator folder's `inbox/new`, written while
# the orchestrator works or idles and never sent into its chat; what the orchestrator sends an
# agent is a `dispatched` file; the person's next message carries every new event after an
# `[Automated:` line and moves the files to `inbox/cur`, a message from an agent pane carries none;
# `tools/on-event` runs with `start` once and with each new file's path, and a failing one is a
# `script_failed` event.
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

# A space's orchestrators share one folder beside the machine's wheel.toml.
FOLDER="$INST/orchestrator/spaces/$A"
mkdir -p "$FOLDER/tools"
printf '#!/bin/sh\necho "$1" >> %s\n' "$SCR/on-event.log" > "$FOLDER/tools/on-event"
chmod +x "$FOLDER/tools/on-event"
new_count() { find "$FOLDER/inbox/new" -type f | grep -c . ; }
cur_count() { find "$FOLDER/inbox/cur" -type f | grep -c . ; }
# kinds: every event's kind, oldest first, new and cur together.
kinds() { find "$FOLDER/inbox/new" "$FOLDER/inbox/cur" -type f | awk -F/ '{print $NF "\t" $0}' | sort | cut -f2 | xargs cat | jq -r .kind | paste -sd, -; }
# wait_new <n>: wait up to 10 s for <n> files in inbox/new.
wait_new() { for _ in $(seq 1 100); do [ "$(new_count)" -ge "$1" ] && return 0; sleep 0.1; done; return 1; }

V="$(w orchestrator start --space "$A")"
expect_out "^$A $V agent:orchestrator-space " "orchestrator start names its pane" w pane ls
w agent wait "$V" --timeout 30 >/dev/null 2>&1
check "the folder holds a Maildir" test -d "$FOLDER/inbox/tmp" -a -d "$FOLDER/inbox/new" -a -d "$FOLDER/inbox/cur"
eq "an empty inbox holds no file" "$(new_count)" "0"
for _ in $(seq 1 50); do grep -qx start "$SCR/on-event.log" 2>/dev/null && break; sleep 0.1; done
check "on-event ran with start" grep -qx start "$SCR/on-event.log"

# Case 1: the orchestrator is mid-conversation (a 6 s turn) when a child agent finishes.
w agent send --timeout 0 "$V" "hold first 6000" >/dev/null
w pane start "$PANE_A" agent --message hello >/dev/null
w pane start "$PANE_B" agent --message hello >/dev/null
CHILD="$(w agent wait "$PANE_A" --timeout 30 | awk '{print $2}')"
w agent wait "$PANE_B" --timeout 30 >/dev/null 2>&1
check "the finished child is a file in inbox/new" wait_new 1
FILE="$(find "$FOLDER/inbox/new" -type f | head -1)"
echo "inbox file $FILE: $(cat "$FILE")"
eq "the file names the child and its kind" "$(jq -r '"\(.agent) \(.kind) \(.text)"' "$FILE")" "$CHILD finished $CHILD finished its turn"
sleep 1
eq "the other space's agent is not in this inbox (no global orchestrator runs)" "$(new_count)" "1"
check "on-event ran with the file's path" grep -qx "$FILE" "$SCR/on-event.log"
MID="$(w agent read "$V" 2>&1)"
check "mid-turn the orchestrator was sent nothing" bash -c "! grep -q 'Automated:' <<<\"\$1\"" _ "$MID"

# Case 2: its turn over and idle, the orchestrator is still sent nothing.
for _ in $(seq 1 120); do
  w agent read "$V" 2>&1 | grep -q "echo: hold first 6000" && break
  sleep 0.1
done
sleep 3
SEEN="$(w agent read "$V" 2>&1)"
check "the idle orchestrator was sent nothing" bash -c "! grep -q 'Automated:' <<<\"\$1\"" _ "$SEEN"

# Case 3: the same event again is a second file; the orchestrator's own send is a dispatch.
w agent send "$CHILD" "hello again" --timeout 30 >/dev/null 2>&1
check "a second finished turn is a second file" wait_new 2
env WHEEL_PANE="$V" "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$INST/token" agent send "$CHILD" "fix the parser" --timeout 30 >/dev/null 2>&1
check "the orchestrator's send and the turn it started are two more files" wait_new 4
eq "three finished turns and one dispatch" "$(kinds | tr , "\n" | sort | paste -sd, -)" "dispatched,finished,finished,finished"
check "the dispatch names the agent and the message" grep -rq "sent $CHILD: fix the parser" "$FOLDER/inbox/new"

# Case 4: a message from an agent pane carries nothing.
env WHEEL_PANE="$PANE_A" "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$INST/token" agent send "$V" "from an agent" --timeout 30 >/dev/null 2>&1
check "an agent pane's message carries no events" bash -c "! '$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' agent read '$V' 2>&1 | grep -q 'Automated:'"
eq "the files still wait in new" "$(new_count)" "4"

# Case 5: the person's next message carries all four and moves them to cur.
env -u WHEEL_PANE "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$INST/token" agent send "$V" "what happened" --timeout 30 >/dev/null 2>&1
READ="$(w agent read "$V" 2>&1)"
echo "the person's message as the orchestrator got it:"; grep -A5 "what happened" <<<"$READ" | head -8
check "the message carries the automated line" grep -q "\[Automated: 4 inbox events from wheeld" <<<"$READ"
eq "inbox/new is empty" "$(new_count)" "0"
eq "inbox/cur holds the four" "$(cur_count)" "4"

# A failing on-event is a script_failed event, for which on-event does not run.
printf '#!/bin/sh\necho broken >&2\nexit 3\n' > "$FOLDER/tools/on-event"
w agent send "$CHILD" "hello once more" --timeout 30 >/dev/null 2>&1
check "the failure is an inbox file" wait_new 2
check "it is script_failed with the exit and stderr" grep -rq "failed: exit 3: broken" "$FOLDER/inbox/new"
has_log "on-event failed" "wheeld logs the failure"

check "a turn limit of 0 is refused" bash -c "! '$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' orchestrator start --space '$A' --turn-limit 0 2>/dev/null"
w orchestrator stop "$V"
check "the folder's events outlive the orchestrator" test "$(cur_count)" -eq 4
finish
