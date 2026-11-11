#!/usr/bin/env bash
# § Wheelhouse Issues and Project views (wheel-cmbu.4): two artifacts served by the one
# `wheelhouse serve` process. The Issues page reads GitHub through gh and shows none of the beads;
# the Project page shows the vision and an edit persists to docs/CONTEXT.md; two spaces see their
# own project. Needs
# `cargo build -p wheelhouse -p wheeld -p wheel-cli`, bd, git. No app.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HOUSE="$BIN/wheelhouse"
[ -x "$HOUSE" ] || { echo "missing $HOUSE: run cargo build -p wheelhouse" >&2; exit 2; }

start_daemon --state-file "$SCR/spaces.json"

make_project() {
  local dir="$SCR/$1"
  mkdir -p "$dir/docs"
  git -C "$SCR" init -q "$dir"
  (cd "$dir" && bd init --prefix "$1" -q >/dev/null 2>&1)
  printf '# %s\n\nThe vision of %s.\n' "$1" "$1" >"$dir/docs/CONTEXT.md"
  echo "$dir"
}
ONE="$(make_project one)"
TWO="$(make_project two)"
(cd "$TWO" && bd create "Only in two" -t task -p 2 >/dev/null 2>&1)
w space new "$ONE" >/dev/null
w space new "$TWO" >/dev/null
SP1="$(w space ls | jget spaces.0.id)"
SP2="$(w space ls | jget spaces.1.id)"

for v in issues project; do
  printf '[artifacts.%s]\nmcp = "exec %s serve"\nshared = true\ntitle = "%s"\n' "$v" "$HOUSE" "$v" >>"$INST/wheel.toml"
done

call() { w artifact call "$1" "$2" ${4:+"$4"} --scope machine --space "$3" 2>&1; }
has() { printf %s "$1" | grep -q -- "$2"; }
hasnt() { ! has "$1" "$2"; }

# Issues reads GitHub through gh, never the beads: a project with no GitHub remote shows gh's error, not its items.
I2="$(call issues get_issues "$SP2")"
check "the Issues page names the gh failure for a project with no GitHub remote" has "$I2" "gh issue list failed"
check "the Issues page lists none of the project's beads" hasnt "$I2" "Only in two"

P1="$(call project get_project "$SP1")"
check "the Project page shows the vision of its own project" has "$P1" "The vision of one."
check "the second space's Project page shows its own vision" has "$(call project get_project "$SP2")" "The vision of two."
call project edit_vision "$SP1" >/dev/null
call project set_vision_draft "$SP1" '{"value":"A vision typed on the page"}' >/dev/null
OUT="$(call project save_vision "$SP1")"
check "the edited vision shows on the page" has "$OUT" "A vision typed on the page"
check "the edit persists to docs/CONTEXT.md" grep -q "A vision typed on the page" "$ONE/docs/CONTEXT.md"
check "the second project's vision is untouched" has "$(cat "$TWO/docs/CONTEXT.md")" "The vision of two."
finish
