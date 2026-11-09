#!/usr/bin/env bash
# § Wheelhouse (wheel-cmbu.2): `wheelhouse serve` is one process that serves every artifact, and
# answers each request from the view the asking artifact names (`_meta` `wheel/artifact`), for the
# project the asking space holds (`wheel/dir`). Two spaces on two projects, each with its own beads,
# share one `beads` entry: each page shows its own project's bead, and one wheelhouse
# process runs. Needs `cargo build -p wheelhouse -p wheeld -p wheel-cli`, `bd` and git. No app.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HOUSE="$BIN/wheelhouse"
[ -x "$HOUSE" ] || { echo "missing $HOUSE: run cargo build -p wheelhouse" >&2; exit 2; }
command -v bd >/dev/null || { echo "bd is not installed" >&2; exit 2; }

start_daemon --state-file "$SCR/spaces.json"

# A project with one bead, named so a page shows whose it is.
make_project() {
  local dir="$SCR/$1"
  mkdir -p "$dir"
  git -C "$dir" init -q
  (cd "$dir" && bd init --prefix "$1" --quiet >/dev/null 2>&1 && bd create "Bead of project $1" -t task -p 2 -l cli >/dev/null 2>&1)
  echo "$dir"
}
ONE="$(make_project one)"
TWO="$(make_project two)"
w space new "$ONE" >/dev/null
w space new "$TWO" >/dev/null
SP1="$(w space ls | jget spaces.0.id)"
SP2="$(w space ls | jget spaces.1.id)"

printf '[artifacts.beads]\nmcp = "exec %s serve"\nshared = true\ntitle = "Beads"\n' "$HOUSE" >"$INST/wheel.toml"

page() { w artifact call beads get_beads --scope machine --space "$1" 2>&1; }
servers() { pgrep -P "$DPID" -f "$HOUSE serve" | wc -l | tr -d ' '; }

has() { printf %s "$1" | grep -q -- "$2"; }
hasnt() { ! has "$1" "$2"; }
tools_of() { w artifact tools beads --scope machine --space "$1" 2>&1; }

P1="$(page "$SP1")"
P2="$(page "$SP2")"
check "the first space's page shows its own bead" has "$P1" "Bead of project one"
check "the second space's page shows its own bead" has "$P2" "Bead of project two"
check "the first page holds nothing of the second project" hasnt "$P1" "Bead of project two"
check "the second page holds nothing of the first project" hasnt "$P2" "Bead of project one"
eq "one wheelhouse process serves both spaces" "$(servers)" 1
check "the artifact lists the Beads view's tools" has "$(tools_of "$SP1")" "set_view"

# A change to a subscribed project's beads reaches its page without a reload. Reading the page's
# resource is what subscribes the host to it.
w artifact read beads a2ui://beads --scope machine --space "$SP1" >/dev/null 2>&1
(cd "$ONE" && bd create "Bead added later" -t task -p 2 -l cli >/dev/null 2>&1)
later() { has "$(page "$SP1")" "Bead added later"; }
check "a bead added to the first project shows on its page within 5 seconds" poll 5 later
check "the second project's page is unchanged by it" hasnt "$(page "$SP2")" "Bead added later"

# An entry that names a view the process does not have is refused by name.
printf '[artifacts.nope]\nmcp = "exec %s serve"\nshared = true\ntitle = "Nope"\n' "$HOUSE" >>"$INST/wheel.toml"
OUT="$(w artifact call nope get_beads --scope machine --space "$SP1" 2>&1)"
check "an artifact no view answers to is refused, naming the views" has "$OUT" "no view named nope"
eq "the refusal came from the same process" "$(servers)" 1
finish
