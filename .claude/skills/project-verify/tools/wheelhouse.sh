#!/usr/bin/env bash
# § Wheelhouse (wheel-cmbu.2): `wheelhouse serve` is one process that serves every artifact, and
# answers each request from the view the asking artifact names (`_meta` `wheel/artifact`), for the
# project the asking space holds (`wheel/dir`). Issues is the one view it serves: two spaces on two
# projects, each with no GitHub remote, share one `issues` entry, and each page names its own
# project's directory in gh's failure; one wheelhouse process runs. Needs
# `cargo build -p wheelhouse -p wheeld -p wheel-cli`, bd and git. No app.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HOUSE="$BIN/wheelhouse"
[ -x "$HOUSE" ] || { echo "missing $HOUSE: run cargo build -p wheelhouse" >&2; exit 2; }

start_daemon --state-file "$SCR/spaces.json"

make_project() {
  local dir="$SCR/$1"
  mkdir -p "$dir"
  git -C "$dir" init -q
  echo "$dir"
}
ONE="$(make_project one)"
TWO="$(make_project two)"
w space new "$ONE" >/dev/null
w space new "$TWO" >/dev/null
SP1="$(w space ls | jget spaces.0.id)"
SP2="$(w space ls | jget spaces.1.id)"

printf '[artifacts.issues]\nmcp = "exec %s serve"\nshared = true\ntitle = "Issues"\n' "$HOUSE" >"$INST/wheel.toml"

page() { w artifact call issues get_issues --scope machine --space "$1" 2>&1; }
servers() { pgrep -P "$DPID" -f "$HOUSE serve" | wc -l | tr -d ' '; }

has() { printf %s "$1" | grep -q -- "$2"; }
hasnt() { ! has "$1" "$2"; }
tools_of() { w artifact tools issues --scope machine --space "$1" 2>&1; }

P1="$(page "$SP1")"
P2="$(page "$SP2")"
check "the first space's page names its own project" has "$P1" "$ONE"
check "the second space's page names its own project" has "$P2" "$TWO"
check "the first page holds nothing of the second project" hasnt "$P1" "$TWO"
check "the second page holds nothing of the first project" hasnt "$P2" "$ONE"
eq "one wheelhouse process serves both spaces" "$(servers)" 1
check "the artifact lists the Issues view's tools" has "$(tools_of "$SP1")" "select_issue"

# An entry that names a view the process does not have is refused by name; Beads is no longer one of them.
printf '[artifacts.beads]\nmcp = "exec %s serve"\nshared = true\ntitle = "Beads"\n' "$HOUSE" >>"$INST/wheel.toml"
OUT="$(w artifact call beads get_beads --scope machine --space "$SP1" 2>&1)"
check "an artifact no view answers to is refused, naming the views" has "$OUT" "no view named beads"
eq "the refusal came from the same process" "$(servers)" 1
finish
