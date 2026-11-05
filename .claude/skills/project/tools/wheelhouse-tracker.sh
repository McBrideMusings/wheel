#!/usr/bin/env bash
# § Wheelhouse Issues and Project views (wheel-cmbu.4): two artifacts served by the one
# `wheelhouse serve` process, writing through bd. An item created from the Issues page is in
# `bd list`; updating and closing it changes `bd show`; the Project page shows the vision and an
# edit persists to docs/CONTEXT.md; two spaces see their own project. Needs
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
  printf '[artifacts.%s-native]\nmcp = "exec %s serve"\nshared = true\ntitle = "%s"\n' "$v" "$HOUSE" "$v" >>"$INST/wheel.toml"
done

call() { w artifact call "$1-native" "$2" ${4:+"$4"} --scope machine --space "$3" 2>&1; }
has() { printf %s "$1" | grep -q -- "$2"; }
hasnt() { ! has "$1" "$2"; }

I1="$(call issues get_issues "$SP1")"
I2="$(call issues get_issues "$SP2")"
check "the Issues page of the second space lists its own item" has "$I2" "Only in two"
check "the first space's Issues page holds nothing of the second project" hasnt "$I1" "Only in two"

call issues set_draft "$SP1" '{"value":"Made from the page"}' >/dev/null
OUT="$(call issues create_issue "$SP1")"
check "creating from the page lists the new item on it" has "$OUT" "Made from the page"
check "the new item is in bd list" has "$(cd "$ONE" && bd list --json 2>&1)" "Made from the page"
ID="$(cd "$ONE" && bd list --json | jget 0.id)"

call issues select_issue "$SP1" "{\"id\":\"$ID\"}" >/dev/null
call issues set_field "$SP1" '{"key":"status","value":"in_progress"}' >/dev/null
check "updating the status from the page shows in bd show" has "$(cd "$ONE" && bd show "$ID" --json 2>&1)" '"in_progress"'
call issues set_field "$SP1" '{"key":"priority","value":"0"}' >/dev/null
check "updating the priority from the page shows in bd show" has "$(cd "$ONE" && bd show "$ID" --json 2>&1)" '"priority": 0'
OUT="$(call issues close_issue "$SP1")"
check "closing from the page shows in bd show" has "$(cd "$ONE" && bd show "$ID" --json 2>&1)" '"closed"'
check "a closed item leaves the Open list" hasnt "$OUT" '"text":"Made from the page"'
call issues select_issue "$SP1" "{\"id\":\"$ID\"}" >/dev/null
OUT="$(call issues set_field "$SP1" '{"key":"status","value":"bogus"}')"
check "an unknown status is refused" has "$OUT" "status must be one of"

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
