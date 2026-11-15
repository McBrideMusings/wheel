#!/bin/sh
# The gate kennel's write actions and file history pass first: an action takes only an item or path kennel lists now,
# the way the pane's feeds list it, in a scope a pane can show: global, or the folder of one of this wheeld's spaces.
# It asks kennel and `wheel` again rather than trusting the caller, prints why it refused on stderr and exits 2. The
# manifest runs it only after refusing a `dir` arg, so it always runs from the package folder.
#
#   sh gate.sh item <item>                          a vault item `kennel vault ls` lists (skills/x, snippets/y.md)
#   sh gate.sh skill <item>                         a vault skill, the only kind `vault install` and `vault copy` take
#   sh gate.sh copy <path>                          a recorded copy `kennel vault ls` lists
#   sh gate.sh listed <path> <scope> <harness>      a skill folder `harness show skills` lists
#   sh gate.sh linked <path> <scope> <harness>      a listed skill folder that is a link into the vault
#   sh gate.sh new <path> <scope> <harness>         a path not there yet, directly inside a skills folder it lists
#   sh gate.sh project <dir>                        a space's folder (`harness show` reads any folder as a project)
#   sh gate.sh harness <name>                       claude, codex or pi
#   sh gate.sh file <path> <section> <scope> <harness> <rev>   a file that section lists (section `vault`: vault ls),
#                                                   and a rev with no `@`, so kennel splits path@rev where the gate did
set -u

refuse() { echo "$1" >&2; exit 2; }

# A scope is `global` or the folder of a space in the wheeld this backend serves.
space_dir() {
    wheel space ls --json | jq -e --arg d "$1" 'any(.[]; .cwd == $d)' > /dev/null || refuse "$1 is not the folder of a space"
}
scope() { [ "$1" = global ] || space_dir "$1"; }
# Callers check the scope first: a refusal inside a pipeline would end only its subshell.
skills() { kennel harness show skills --scope "$1" --harness "$2" --json; }

case "${1:-}" in
item)
    kennel vault ls --json | jq -e --arg i "$2" '[.skills[], .snippets[]] | any(.item == $i)' > /dev/null ||
        refuse "$2 is not an item in the vault" ;;
skill)
    kennel vault ls --json | jq -e --arg i "$2" '.skills | any(.item == $i)' > /dev/null ||
        refuse "$2 is not a skill in the vault" ;;
copy)
    kennel vault ls --json | jq -e --arg p "$2" '.copies | any(.path == $p)' > /dev/null ||
        refuse "$2 is not a copy kennel recorded" ;;
listed)
    scope "$3"
    skills "$3" "$4" | jq -e --arg p "$2" '[.blocks[].entries[]] | any(.path == $p)' > /dev/null ||
        refuse "$2 is not a skill kennel lists for $4 in $3" ;;
linked)
    scope "$3"
    skills "$3" "$4" | jq -e --arg p "$2" '[.blocks[].entries[]] | any(.path == $p and .vault)' > /dev/null ||
        refuse "$2 is not a link into the vault kennel lists for $4 in $3" ;;
new)
    case "$2" in */../* | */./* | */.. | */. | *//* | */) refuse "$2 is not a plain path" ;; esac
    scope "$3"
    skills "$3" "$4" | jq -e --arg d "$(dirname "$2")" '[.blocks[].dirs[]] | any(. == $d)' > /dev/null ||
        refuse "$2 is not in a skills folder kennel lists for $4 in $3"
    [ -e "$2" ] || [ -L "$2" ] && refuse "$2 already exists"
    exit 0 ;;
project)
    space_dir "$2" ;;
harness)
    case "$2" in claude | codex | pi) ;; *) refuse "$2 is not a harness (claude, codex or pi)" ;; esac ;;
file)
    case "${6:-}" in *@*) refuse "${6} is not a revision kennel can read after a path" ;; esac
    if [ "$3" = vault ]; then
        kennel vault ls --json | jq -e --arg p "$2" \
            '[(.skills[] | .path as $d | .files[] | $d + "/" + .), (.snippets[] | .path)] | any(. == $p)' > /dev/null
    else
        scope "$4"
        kennel harness show "$3" --scope "$4" --harness "$5" --json | jq -e --arg p "$2" '[.blocks[].entries[] |
            (if .files then .path as $d | .files[] | $d + "/" + . else .path end), .file, .script]
            | any(. == $p)' > /dev/null
    fi || refuse "$2 is not a file kennel lists" ;;
*)
    refuse "gate.sh: no check named ${1:-(none)}" ;;
esac
