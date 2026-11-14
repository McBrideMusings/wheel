#!/usr/bin/env bash
# § Wheelhouse Diff and PRs views (wheel-cmbu.3): two artifacts served by the one `wheelhouse serve`
# process. Two spaces on two clones each see their own diff; a file edited in a subscribed project
# reaches its Diff page without a reload. The Repo view is the wheelhouse plugin's `repo` pane,
# checked by tools/wheelhouse-plugin.sh. Needs `cargo build -p wheelhouse -p wheeld -p wheel-cli`, git. No app.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HOUSE="$BIN/wheelhouse"
[ -x "$HOUSE" ] || { echo "missing $HOUSE: run cargo build -p wheelhouse" >&2; exit 2; }

start_daemon --state-file "$SCR/spaces.json"

# A clone of a bare origin with `main` pushed and a branch `done-<name>` already merged and pushed.
make_project() {
  local dir="$SCR/$1"
  git init -q --bare "$SCR/$1.git"
  git clone -q "$SCR/$1.git" "$dir" 2>/dev/null
  git -C "$dir" checkout -q -b main
  echo "$1" >"$dir/readme"
  git -C "$dir" add readme
  git -C "$dir" -c user.name=t -c user.email=t@t commit -q -m "first of $1"
  git -C "$dir" push -q origin main
  git -C "$dir" remote set-head origin main >/dev/null 2>&1
  git -C "$dir" branch "done-$1"
  git -C "$dir" push -q origin "done-$1"
  echo "$dir"
}
ONE="$(make_project one)"
TWO="$(make_project two)"
w space new "$ONE" >/dev/null
w space new "$TWO" >/dev/null
SP1="$(w space ls | jget spaces.0.id)"
SP2="$(w space ls | jget spaces.1.id)"

for v in diff prs; do
  printf '[artifacts.%s]\nmcp = "exec %s serve"\nshared = true\ntitle = "%s"\n' "$v" "$HOUSE" "$v" >>"$INST/wheel.toml"
done

call() { w artifact call "$1" "$2" ${4:+"$4"} --scope machine --space "$3" 2>&1; }
servers() { pgrep -P "$DPID" -f "$HOUSE serve" | wc -l | tr -d ' '; }
has() { printf %s "$1" | grep -q -- "$2"; }
hasnt() { ! has "$1" "$2"; }

D1="$(call diff get_diff "$SP1")"
check "the Diff page of a clean checkout says there are no uncommitted changes" has "$D1" "No uncommitted changes"

P1="$(call prs get_prs "$SP1")"
check "the PRs page answers with its title" has "$P1" "PRs"
eq "one wheelhouse process serves both artifacts in both spaces" "$(servers)" 1

# A file edited in a subscribed project reaches its Diff page without a reload.
w artifact read diff a2ui://diff --scope machine --space "$SP1" >/dev/null 2>&1
echo "changed" >>"$ONE/readme"
changed() { has "$(call diff get_diff "$SP1")" "readme"; }
check "an edit to a tracked file shows on the Diff page within 5 seconds" poll 5 changed
check "the second project's Diff page is unchanged by it" has "$(call diff get_diff "$SP2")" "No uncommitted changes"
finish
