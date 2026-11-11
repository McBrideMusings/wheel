#!/usr/bin/env bash
# § Wheelhouse list actions (wheel-xxp1): the Beads page's Copy ids button carries the ids the
# page lists now, in the order shown; a header click reaches the page (set_sort) and re-orders
# both the rows and the ids; a project's `[beads.buttons.<name>]` with `on = ["list"]` shows in
# the bar and runs with BEAD_IDS; a row button never shows as a list button. Drives the real
# `wheel` against this script's own daemon through `artifact call`. Needs `cargo build -p
# wheelhouse -p wheeld -p wheel-cli`, bd, jq. No app.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HOUSE="$BIN/wheelhouse"
[ -x "$HOUSE" ] || { echo "missing $HOUSE: run cargo build -p wheelhouse" >&2; exit 2; }
command -v bd >/dev/null || { echo "bd is not installed" >&2; exit 2; }
command -v jq >/dev/null || { echo "jq is not installed" >&2; exit 2; }

start_daemon --state-file "$SCR/spaces.json"
export WHEEL_URL="ws://127.0.0.1:$PORT/ws" WHEEL_TOKEN_FILE="$INST/token"

PROJ="$SCR/proj"; mkdir -p "$PROJ"
(cd "$PROJ" && bd init --prefix lt --quiet >/dev/null 2>&1 \
  && bd create "Alpha" -t task -p 3 -l cli >/dev/null 2>&1 \
  && bd create "Beta" -t task -p 1 -l cli >/dev/null 2>&1 \
  && bd create "An epic" -t epic -p 2 -l cli >/dev/null 2>&1)
A="$(cd "$PROJ" && bd list --json | jq -r '.[] | select(.title=="Alpha") | .id')"
B="$(cd "$PROJ" && bd list --json | jq -r '.[] | select(.title=="Beta") | .id')"
E="$(cd "$PROJ" && bd list --json | jq -r '.[] | select(.title=="An epic") | .id')"
# A list button that records what it was given, and a row button that must never show in the bar.
cat >"$PROJ/wheel.toml" <<EOF
[beads.buttons.record]
label = "Record"
on = ["list"]
run = 'printf "%s|%s|%s" "\$BEAD_IDS" "\$BEAD_REPO" "\$WHEEL_PANE" > "$SCR/record.txt"'
[beads.buttons.one]
label = "One"
on = ["ready"]
run = "true"
EOF

SP="$(w space new "$PROJ")"
PANE="$(w space ls | jq -r --arg s "$SP" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
printf '[artifacts.beads]\nmcp = "exec %s serve"\nshared = true\ntitle = "Beads"\n' "$HOUSE" >>"$INST/wheel.toml"

call() {
  local out; out="$(w artifact call beads "$1" ${2:+"$2"} --scope machine --space "$SP" --pane "$PANE" 2>&1)"
  printf %s "$out" | jq -r '.resource.text' 2>/dev/null || printf %s "$out"
}
has_text() { printf %s "$1" | grep -q -- "$2"; }
copied() { printf %s "$1" | jq -r '[.. | objects | select(.id? == "list.copy")][0].action.copy.text'; }
disabled() { printf %s "$1" | jq -r --arg id "$2" '[.. | objects | select(.id? == $id)][0].modifiers // [] | any(.disabled? == true)'; }
rows() { printf %s "$1" | jq -r '[.. | objects | select(.id? == "table")][0].sections[0].rows | map(.id) | join(" ")'; }
shape() { printf %s "$1" | jq -r '[.. | objects | select(.id? == "bar.shape")][0].children | join(" ")'; }

PAGE="$(call get_beads)"
eq "the bar holds Clear, Copy ids, the list button and the count" "$(shape "$PAGE")" "filters.clear list.copy list.record count"
eq "Copy ids holds the two open leaves and not the epic" "$(copied "$PAGE")" "$(rows "$PAGE")"
check "the copied ids are the leaves" test "$(copied "$PAGE" | tr ' ' '\n' | sort | tr '\n' ' ')" = "$(printf '%s\n%s\n' "$A" "$B" | sort | tr '\n' ' ')"
eq "a header click's sort reaches the table" "$(printf %s "$PAGE" | jq -r '[.. | objects | select(.id? == "table")][0] | .sortAction + " " + .sort.key + " " + (.sort.asc|tostring)')" "set_sort status true"

PAGE="$(call set_sort '{"key":"pri","asc":true}')"
eq "sorted by priority ascending: Beta (P1) before Alpha (P3)" "$(rows "$PAGE")" "$B $A"
eq "and the copied ids follow" "$(copied "$PAGE")" "$B $A"
PAGE="$(call set_sort '{"key":"pri","asc":false}')"
eq "priority descending flips both" "$(copied "$PAGE")" "$A $B"
PAGE="$(call set_sort '{"key":"nonsense","asc":true}')"
eq "an unknown column leaves the sort alone" "$(printf %s "$PAGE" | jq -r '[.. | objects | select(.id? == "table")][0].sort.key')" "pri"

OUT="$(call run_list_script '{"script":"record"}')"
check "the list button reports what it started" has_text "$OUT" "Record started over 2 beads"
poll 5 test -s "$SCR/record.txt"
eq "the script got the ids in the shown order, the repo and the pane" "$(cat "$SCR/record.txt")" "$A $B|proj|$PANE"
OUT="$(call run_list_script '{"script":"one"}')"
check "a row button is refused as a list button" has_text "$OUT" "no list button one"

PAGE="$(call set_query '{"q":"nothing-here"}')"
eq "an empty list copies nothing" "$(copied "$PAGE")" ""
eq "and keeps Copy ids in place, disabled" "$(disabled "$PAGE" list.copy)" "true"
eq "and the list button too" "$(disabled "$PAGE" list.record)" "true"
OUT="$(call run_list_script '{"script":"record"}')"
check "the list button refuses an empty list" has_text "$OUT" "the list is empty"

finish
