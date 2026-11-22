#!/usr/bin/env bash
# The orchestrator's inbox (wheel-eg4e.20.1, ADR-0054), through the wheel command against the fake
# agent: a Maildir in the orchestrator's folder ($INST/orchestrator/spaces/<space> for a space
# orchestrator). A routed child's finished turn, a failed build, and each message the orchestrator
# sends from its pane land as one JSON file each in inbox/new; tools/on-event runs once at the start
# and once per file, with the file's path and the event on its stdin, and its failure is logged and written as an on_event_failed file; wheeld writes
# nothing into the orchestrator's chat until the person sends it a message, which carries the new
# files inside <wheel-inbox automated="true"> and moves them to inbox/cur.
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
# person <args>: the wheel command as the person runs it, from no pane.
person() { env -u WHEEL_PANE "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$INST/token" "$@"; }

DIR="$INST/orchestrator/spaces/$A"
mkdir -p "$DIR/tools"
printf '#!/bin/sh\nbasename "$1" >> "$WHEEL_ORCHESTRATOR_DIR/ran.log"\n' >"$DIR/tools/on-event"
chmod +x "$DIR/tools/on-event"

V="$(person orchestrator start --space "$A")"
expect_out "^$A $V agent:orchestrator-space " "orchestrator start names its pane" w pane ls
person agent wait "$V" --timeout 30 >/dev/null 2>&1
# events <kind|any>: the inbox files in new and cur routed to $V, as `kind agent: text`, oldest first.
events() {
  find "$DIR/inbox/new" "$DIR/inbox/cur" -name '*.json' 2>/dev/null | awk -F/ '{print $NF "\t" $0}' | sort | cut -f2 \
    | xargs cat 2>/dev/null | jq -r --arg p "$V" --arg k "$1" 'select(.orchestrator == $p and ($k == "any" or .kind == $k)) | "\(.kind) \(.agent // "-"): \(.text)"'
}
# wait_for <pattern>: up to 10 s for an event line matching it.
wait_for() {
  for _ in $(seq 1 100); do events any | grep -q "$1" && return 0; sleep 0.1; done
  return 1
}
check "the start event is written once the orchestrator registers" wait_for "^start -: wheeld started"

# The orchestrator is mid-conversation (a 6 s turn) when a child agent finishes.
person agent send --timeout 0 "$V" "hold first 6000" >/dev/null
person pane start "$PANE_A" agent --message hello >/dev/null
person pane start "$PANE_B" agent --message hello >/dev/null
CHILD="$(person agent wait "$PANE_A" --timeout 30 | awk '{print $2}')"
check "the finished child is one inbox file" wait_for "^finished $CHILD: $CHILD finished its turn$"
eq "the other space's agent is not in this inbox (no global orchestrator runs)" "$(events finished | grep -vc "^finished $CHILD:")" "0"
MID="$(person agent read "$V" 2>&1)"
check "mid-turn the orchestrator was sent nothing" bash -c "! grep -q 'finished its turn' <<<\"\$1\"" _ "$MID"

# A message from the orchestrator's pane is a dispatch; a failing `make build` ("show tools") is
# its own kind.
WHEEL_PANE="$V" "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$INST/token" agent send "$CHILD" "show tools" --timeout 30 >/dev/null 2>&1
check "the orchestrator's message is a dispatched event" wait_for "^dispatched $CHILD: dispatched to $CHILD: show tools$"
check "the failed build is a build_failed event" wait_for "^build_failed $CHILD: $CHILD ran a build or test that failed: make build"
check "nothing is left half-written in inbox/tmp" bash -c "[ -z \"\$(ls -A '$DIR/inbox/tmp')\" ]"
echo "inbox:"; events any

# tools/on-event ran once per file, oldest first (the start file went with "hold first", so it is in cur).
files() { { ls "$DIR/inbox/new"; ls "$DIR/inbox/cur"; } | sort; }
for _ in $(seq 1 100); do
  [ "$(wc -l <"$DIR/ran.log" 2>/dev/null)" -ge "$(files | wc -l)" ] && break
  sleep 0.1
done
eq "tools/on-event ran once per file, in order" "$(cat "$DIR/ran.log")" "$(files)"

# Idle, the orchestrator is still sent nothing; the person's message carries the new files.
for _ in $(seq 1 120); do
  person agent read "$V" 2>&1 | grep -q "echo: hold first 6000" && break
  sleep 0.1
done
SEEN="$(person agent read "$V" 2>&1)"
check "the idle orchestrator was sent nothing" bash -c "! grep -q 'finished its turn' <<<\"\$1\"" _ "$SEEN"
person agent send "$V" "what happened?" --timeout 30 >/dev/null 2>&1
SEEN="$(person agent read "$V" 2>&1)"
check "the person's message carries the inbox, marked automated" grep -q '<wheel-inbox automated="true">' <<<"$SEEN"
check "it names the child's finished turn" grep -q "finished: $CHILD finished its turn" <<<"$SEEN"
eq "every attached file moved to inbox/cur" "$(ls "$DIR/inbox/new" | wc -l | tr -d ' ')" "0"

# A run still queued, or running, when the person's message moves its file to cur reads the event
# from its stdin.
printf '#!/bin/sh\nsleep 1\njq -e .kind >/dev/null || { echo "no event on stdin" >&2; exit 4; }\nbasename "$1" >> "$WHEEL_ORCHESTRATOR_DIR/slow.log"\n' >"$DIR/tools/on-event"
WHEEL_PANE="$V" "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$INST/token" agent send "$CHILD" "hello slow" --timeout 30 >/dev/null 2>&1
person agent send --timeout 0 "$V" "and now?" >/dev/null
for _ in $(seq 1 100); do [ "$(wc -l <"$DIR/slow.log" 2>/dev/null)" -ge 2 ] && break; sleep 0.1; done
eq "both runs read their event after its file moved to cur" "$(wc -l <"$DIR/slow.log" | tr -d ' ')" "2"
eq "and none failed" "$(events on_event_failed | wc -l | tr -d ' ')" "0"

# A failing tools/on-event is logged and written as an on_event_failed file, which runs nothing.
printf '#!/bin/sh\necho boom >&2\nexit 3\n' >"$DIR/tools/on-event"
WHEEL_PANE="$V" "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$INST/token" agent send "$CHILD" "hello again" --timeout 30 >/dev/null 2>&1
check "the script's failure is an inbox event" wait_for "^on_event_failed -: tools/on-event failed on .*exit status: 3; stderr: boom"
check "and a line in wheeld's log" grep -rq "tools/on-event failed" "$LOGS"

check "a turn limit of 0 is refused" bash -c "! '$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' orchestrator start --space '$A' --turn-limit 0 2>/dev/null"
check "wheel inbox is gone" bash -c "! '$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' inbox list '$V' 2>/dev/null"
person orchestrator stop "$V"
finish
