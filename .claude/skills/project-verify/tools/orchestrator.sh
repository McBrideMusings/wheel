#!/usr/bin/env bash
# Orchestrators (wheel-eg4e.19.4, ADR-0054), through the wheel command against the fake agent.
# `wheel orchestrator start` makes an ordinary agent pane with a scope and prints its pane; several
# may share a space and its folder; each agent event lands as a file in the inbox of the agent's
# space's folder when a space orchestrator runs there, else in the global folder (the Orchestrator
# space's); a space orchestrator's own events land in the global folder;
# `wheel orchestrator stop <pane>` closes the pane. A global orchestrator goes in the Orchestrator
# space (wheel-eg4e.19.9), made at the first global start in a session of its own.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
trap cleanup EXIT

use_fake_agent
start_daemon --state-file "$SCR/state.json"
mkdir -p "$SCR/alpha" "$SCR/beta" "$SCR/top"
A="$(w space new "$SCR/alpha")"
B="$(w space new "$SCR/beta")"
T="$(w space new "$SCR/top")"
first_pane() { w space ls | jq -r --arg s "$1" '[.spaces[] | select(.id == $s)][0] | [.. | objects | select(.id? and .kind?) | .id] | first'; }
PANE_A="$(first_pane "$A")"
PANE_B="$(first_pane "$B")"
fails() { ! "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$INST/token" "$@" >/dev/null 2>&1; }

check "a space that does not exist is refused" fails orchestrator start --space s-missing
O1="$(w orchestrator start --space "$A")"
expect_out "^$A $O1 agent:orchestrator-space " "case 1: pane ls shows an agent pane marked orchestrator" w pane ls
O2="$(w orchestrator start --space "$A")"
check "case 2: a second orchestrator in the space is another pane" test "$O2" != "$O1"
FOLDER_A="$INST/orchestrator/spaces/$A"
check "case 2: the two share the space's folder" test -d "$FOLDER_A/inbox/new"

# agents_in <folder>: the agents of every event in <folder>'s inbox, sorted, one per event.
agents_in() { find "$1/inbox/new" "$1/inbox/cur" -type f 2>/dev/null | xargs cat 2>/dev/null | jq -r .agent | sort | paste -sd, -; }
# inbox_has <folder> <agent>: wait up to 10 s for an event about <agent> in <folder>'s inbox.
inbox_has() {
  for _ in $(seq 1 100); do
    agents_in "$1" | tr , '\n' | grep -qx "$2" && return 0
    sleep 0.1
  done
  return 1
}

# Case 6: no global orchestrator yet; a space orchestrator's finished turn reaches nobody.
w agent send "$O1" hello --timeout 30 >/dev/null 2>&1
w pane start "$PANE_A" agent --message hello >/dev/null
CHILD_A="$(w agent wait "$PANE_A" --timeout 30 | awk '{print $2}')"
check "case 3: this space's agent lands in the space's folder" inbox_has "$FOLDER_A" "$CHILD_A"
sleep 0.5
eq "case 3 and 6: once, and not o1's own turn" "$(agents_in "$FOLDER_A")" "$CHILD_A"

check "a global orchestrator naming a space is refused" fails orchestrator start --global --space "$T"
G1="$(w orchestrator start --global)"
ORCH="$(w space ls | jq -r '[.spaces[] | select(.orchestrator == true)] | if length == 1 then .[0].id else "none" end')"
expect_out "^$ORCH $G1 agent:orchestrator-global " "wheel-eg4e.19.9: a global orchestrator goes in the one Orchestrator space" w pane ls
ORCH_CWD="$(w space ls | jq -r --arg s "$ORCH" '.spaces[] | select(.id == $s) | .cwd')"
check "wheel-eg4e.19.9: the Orchestrator space's folder exists ($ORCH_CWD)" test -d "$ORCH_CWD"
eq "wheel-eg4e.19.9: its session is Orchestrator and holds only it" "$(w session ls --json | jq -r --arg s "$ORCH" '.[] | select(.spaces | index($s)) | "\(.name) \(.spaces | length)"')" "Orchestrator 1"
check "wheel-eg4e.19.9: the Orchestrator space cannot be closed" fails space close "$ORCH"
w pane start "$PANE_B" agent --message hello >/dev/null
CHILD_B="$(w agent wait "$PANE_B" --timeout 30 | awk '{print $2}')"
check "case 4: a space with no orchestrator reports to the global folder" inbox_has "$ORCH_CWD" "$CHILD_B"
O1_AGENT="$(w agent wait "$O1" --timeout 30 | awk '{print $2}')"
w agent send "$O1" "hello again" --timeout 30 >/dev/null 2>&1
check "case 5: the space orchestrator's own turn lands in the global folder" inbox_has "$ORCH_CWD" "$O1_AGENT"
eq "case 5: not in its own space's folder" "$(agents_in "$FOLDER_A")" "$CHILD_A"

# Case 7: the orchestrator's pane runs the whole CLI: it closes a pane in another space.
w pane split "$PANE_B" right >/dev/null
SPARE="$(w pane ls | awk -v s="$B" -v p="$PANE_B" '$1 == s && $2 != p {print $2}' | tail -1)"
check "case 7: a command outside the old list runs from the orchestrator's pane" env WHEEL_PANE="$O1" "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$INST/token" pane close "$SPARE"
eq "the pane in the other space is gone" "$(w pane ls | grep -c " $SPARE ")" "0"

w orchestrator stop "$O1"
eq "orchestrator stop closes its pane" "$(w pane ls | grep -c " $O1 ")" "0"
check "a second stop is refused" fails orchestrator stop "$O1"
check "a pane that is no orchestrator is refused" fails orchestrator stop "$PANE_A"
finish
