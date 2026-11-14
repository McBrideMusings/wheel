#!/usr/bin/env bash
# § Wheelhouse PRs view (wheel-cmbu.3): the `prs` artifact served by `wheelhouse serve`, one process
# for two spaces on two clones. Diff and Repo are wheelhouse plugin panes, checked by
# tools/wheelhouse-plugin.sh. Needs `cargo build -p wheelhouse -p wheeld -p wheel-cli`, git. No app.
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

for v in prs; do
  printf '[artifacts.%s]\nmcp = "exec %s serve"\nshared = true\ntitle = "%s"\n' "$v" "$HOUSE" "$v" >>"$INST/wheel.toml"
done

call() { w artifact call "$1" "$2" ${4:+"$4"} --scope machine --space "$3" 2>&1; }
servers() { pgrep -P "$DPID" -f "$HOUSE serve" | wc -l | tr -d ' '; }
has() { printf %s "$1" | grep -q -- "$2"; }
hasnt() { ! has "$1" "$2"; }

P1="$(call prs get_prs "$SP1")"
check "the PRs page answers with its title" has "$P1" "PRs"
eq "one wheelhouse process serves the artifact in both spaces" "$(servers)" 1

finish
