#!/usr/bin/env bash
# § Wheelhouse agents (wheel-cmbu.5): the core starts an agent pane in the space that asked, with
# a brief and no focus change, and reports each agent's state from `wheel agent watch`, which
# prints a new list when the daemon announces a change, so a reader never polls. Drives the real
# `wheel` and `wheelhouse agents` against this script's own daemon (a fake agent: a brief that says
# "permission" leaves the agent waiting on a permission). Needs `cargo build -p wheelhouse -p wheeld
# -p wheel-cli --examples --bins`. No app.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HOUSE="$BIN/wheelhouse"
[ -x "$HOUSE" ] || { echo "missing $HOUSE: run cargo build -p wheelhouse" >&2; exit 2; }
command -v jq >/dev/null || { echo "jq is not installed" >&2; exit 2; }

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"

# The children below reach this script's daemon, not the one on the default port.
export WHEEL_URL="ws://127.0.0.1:$PORT/ws" WHEEL_TOKEN_FILE="$INST/token"
export PATH="$(dirname "$WHEEL"):$PATH"

mkdir -p "$SCR/proj" "$SCR/other"
SP="$(w space new "$SCR/proj")"
OTHER="$(w space new "$SCR/other")"
PANE="$(w space ls | jq -r --arg s "$SP" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
panes_of() { w space ls | jq -r --arg s "$1" '[.spaces[] | select(.id==$s) | .root | .. | objects | select(.node=="pane") | .id] | join(" ")'; }

# A reader that is waiting when the change happens: every list `agent watch` prints goes to a file.
LISTS="$SCR/lists"
: >"$LISTS"
bg w agent watch >"$LISTS"
poll 5 test "$(wc -l <"$LISTS")" -ge 1
eq "the first list has no agent in the new space" "$(tail -1 "$LISTS" | jq --arg d "$SCR/proj" '[.[] | select(.cwd==$d)] | length')" 0

NEW="$(cd "$SCR/proj" && "$HOUSE" agents spawn "permission please" --pane "$PANE" --model haiku 2>&1)"
check "spawn prints the new pane's id" test -n "$NEW" -a "$NEW" != "$PANE"
case " $(panes_of "$SP") " in *" $NEW "*) ok "the new pane is in the space that asked" ;; *) fail "the new pane is in the space that asked" "panes: $(panes_of "$SP")" ;; esac
case " $(panes_of "$OTHER") " in *" $NEW "*) fail "the other space is untouched" ;; *) ok "the other space is untouched" ;; esac
eq "the pane runs the model asked for" "$(w status --json | jq -r --arg p "$NEW" '[.spaces[].root | .. | objects | select(.id? == $p)][0].model')" haiku
check "the brief reached the agent" bash -c "'$WHEEL' --url '$WHEEL_URL' --token-file '$INST/token' agent read '$NEW' | grep -q 'permission please'"

# The watcher saw the new pane waiting on a permission, with no poll of ours in between.
needs() { tail -1 "$LISTS" | jq -e --arg p "$NEW" '.[] | select(.pane==$p and .status=="needs_you")' >/dev/null; }
check "a watcher is told the new agent waits on a permission" poll 10 needs
eq "its row names the space's directory" "$(tail -1 "$LISTS" | jq -r --arg p "$NEW" '.[] | select(.pane==$p) | .cwd')" "$SCR/proj"

# `wheelhouse agents ls` reads the same list for one project.
LS="$(cd "$SCR/proj" && "$HOUSE" agents ls)"
eq "ls lists the project's agent" "$(printf %s "$LS" | jq -r '.[0].pane')" "$NEW"
eq "ls reports it waiting" "$(printf %s "$LS" | jq -r '.[0].status')" needs_you

# An answer changes the state and the watcher hears it.
w perm answer "$NEW" allow >/dev/null
idle() { tail -1 "$LISTS" | jq -e --arg p "$NEW" '.[] | select(.pane==$p and .status=="idle")' >/dev/null; }
check "answering the permission shows the agent idle" poll 10 idle

# A spawn that cannot start the agent leaves no empty pane behind.
BEFORE="$(panes_of "$SP")"
OUT="$(cd "$SCR/proj" && "$HOUSE" agents spawn "hi" --pane "$PANE" --agent no-such-agent 2>&1)"; RC=$?
check "an unknown agent is refused" test "$RC" -ne 0
eq "the refused spawn left the space's panes as they were" "$(panes_of "$SP")" "$BEFORE"
finish
