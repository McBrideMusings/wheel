#!/usr/bin/env bash
# § The artifact picker shows an entry's one-line description (wheel-k5yn): the pane picker's rows carry
# `description` in the Mac app's /state, a row without one lists as before, a long one is clipped by the
# row rather than the data, and a description over two lines is a problem that leaves it unset. Drives
# the real Mac app against this script's own daemon in one space.
# WHEEL_SHOTS=<dir> keeps the pane snapshot (set WHEEL_APPEARANCE=light|dark for the theme).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"
LOOK="${WHEEL_APPEARANCE:-light}"

start_daemon --state-file "$SCR/spaces.json"
mkdir -p "$SCR/proj1"
S1="$(w space new "$SCR/proj1")"
LONG="Shows every build the swarm has run today with its result, the worker that ran it, how long it took and the commit it built, refreshed each minute."

w artifact new described --pane --space "$S1" --description "Shows the swarm's builds." >/dev/null
w artifact new plain --pane --space "$S1" >/dev/null
w artifact new longone --pane --space "$S1" --description "$LONG" >/dev/null
w artifact new onmachine --pane --scope machine --description "A viewer every project can open." >/dev/null
printf '\n[artifacts.twolines]\npath = ".wheel/artifacts/described"\npane = true\ndescription = """first\nsecond"""\n' >>"$SCR/proj1/wheel.toml"

echo "     ls --json: $(w artifact ls --space "$S1" --json | jq -c "(.artifacts // .) | [.[] | {name, description}]")"
eq "ls --json gives the described entry its description" "$(w artifact ls --space "$S1" --json | jq -r "(.artifacts // .) | .[] | select(.name == \"described\") | .description")" "Shows the swarm's builds."
eq "ls --json gives the entry with none null" "$(w artifact ls --space "$S1" --json | jq -r "(.artifacts // .) | .[] | select(.name == \"plain\") | .description")" "null"

prefs_reset
launch_app
wait_st '.windows | length >= 1' 20
w space focus "$S1" >/dev/null 2>&1 || true
P="$(w space ls | jq -r --arg s "$S1" '[.spaces[] | select(.id == $s)][0] | [.. | objects | select(.id? and .kind?) | .id] | first')"
w pane kind "$P" empty >/dev/null 2>&1 || true

app_post "/palette?pane=$P&action=open" >/dev/null
check "the picker is ready" wait_st ".panes[\"$P\"].palette.state == \"ready\"" 10
rows() { st -c --arg p "$P" '[.panes[$p].palette.rows[] | {name, description}]'; }
echo "     rows: $(rows)"
desc() { st -r --arg p "$P" --arg n "$1" '.panes[$p].palette.rows[] | select(.name == $n) | .description'; }
eq "the described row carries its description" "$(desc described)" "Shows the swarm's builds."
eq "the row without one carries none" "$(desc plain)" "null"
eq "the long description is whole in the data" "$(desc longone)" "$LONG"
eq "the machine row carries its description" "$(desc onmachine)" "A viewer every project can open."
eq "a two-line description is a problem and no description" "$(desc twolines)|$(st -r --arg p "$P" '.panes[$p].palette.rows[] | select(.name == "twolines") | .problems[0]')" "null|description is more than one line"
app_post "/window/snapshot?path=$SHOTS/picker-description-$LOOK.png" >/dev/null && echo "     shot $SHOTS/picker-description-$LOOK.png"
app_post "/palette?pane=$P&action=close" >/dev/null

# Narrow: two splits leave the last pane under 420 pt wide, where the row clips to one line.
lastpane() { w space ls | jq -r --arg s "$S1" '[.spaces[] | select(.id == $s)][0] | [.. | objects | select(.id? and .kind?) | .id] | last'; }
Q="$P"; for _ in 1 2; do w pane split "$Q" right >/dev/null; Q="$(lastpane)"; done
w pane kind "$Q" empty >/dev/null 2>&1 || true
app_post "/palette?pane=$Q&action=open" >/dev/null
check "the narrow picker is ready" wait_st ".panes[\"$Q\"].palette.state == \"ready\"" 10
app_post "/window/snapshot?path=$SHOTS/picker-description-narrow-$LOOK.png" >/dev/null && echo "     shot $SHOTS/picker-description-narrow-$LOOK.png"
app_post "/palette?pane=$Q&action=close" >/dev/null

finish
