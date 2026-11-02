#!/usr/bin/env bash
# § Edits card (wheel-h3z5.21): the card ending a turn that edited files lists the first
# chat.edits.shown files (default 3) and folds the rest behind "… N more files", whose click lists
# every file and reads "Show fewer"; each file row opens its own diff, and "Collapse all" closes
# every open one. The fake agent's "edit N files" turn edits N files (+2 -1 each). Drives the real
# Mac app through its debug server against this script's own daemon; /state lists each card as
# `edits N files · listed <path +a -r, …> [· more "…" | · show fewer] · open [paths] · collapse_all
# shown|hidden @<id> [open]`. /agent/expand?id=<card> is the click on the fold row, with `path=` the
# click on a file's row, with `collapse=all` the click on "Collapse all". Window shots
# (cacheDisplay) go to $WHEEL_SHOTS (default $SCR/shots), light and dark.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"

export CLAUDE_CONFIG_DIR="$SCR/claude"
use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
WHEEL_APPEARANCE=light

mkdir -p "$SCR/a"
A="$(w space new "$SCR/a")"
P="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | .root.id')"
w pane start "$P" agent >/dev/null

launch_app
check "the app connects" wait_st '.connection == "connected"' 20
app_post "/focus?space=$A" >/dev/null

for n in 1 3 4 20; do
  w agent send "$P" "edit $n files" >/dev/null
  w agent wait "$P" >/dev/null
done
items() { app_get /state | jq -r --arg p "$P" '.panes[$p].transcript_items[]?'; }
# card N: the state line of the edits card of the turn that edited N files.
card() { items | grep "^edits $1 files " | head -1; }
card_id() { card "$1" | sed 's/.* @\(-\{0,1\}[0-9]*\).*/\1/'; }
has() { card "$1" | grep -F -- "$2" >/dev/null; }
hasnt() { ! has "$@"; }
check "all four cards reached the app" wait_st ".panes[\"$P\"].transcript_items | map(select(startswith(\"edits \"))) | length == 4" 15
echo "--- cards:"; items | grep '^edits '

AG="$(app_get /state | jq -r --arg p "$P" '.panes[$p].agent // empty')"
check "the pane's agent name is known" test -n "$AG"
click() { app_post "/agent/expand?agent=$AG&id=$1${2:+&$2}" >/dev/null || echo "click refused: $*"; sleep 0.5; }
clickp() { click "$1" "path=$(printf %s "$2" | jq -sRr @uri)"; }

check "1. one file: listed, no … row" has 1 'listed src/file-1.go +2 -1 · open'
check "1. one file: no fold row" hasnt 1 'more'
check "2. three files: all listed" has 3 'listed src/file-1.go +2 -1, src/file-2.go +2 -1, src/file-3.go +2 -1 · open'
check "2. three files: no … row" hasnt 3 'more'
check "3. four files: three listed, then … 1 more file" has 4 'listed src/file-1.go +2 -1, src/file-2.go +2 -1, src/file-3.go +2 -1 · more "… 1 more file +2 -1"'
check "4. twenty: three listed, then … 17 more files with +34 -17" has 20 'listed src/file-1.go +2 -1, src/file-2.go +2 -1, src/file-3.go +2 -1 · more "… 17 more files +34 -17"'
check "8. the header's counts are the whole turn's" has 20 'edits 20 files'

shot() { app_post "/window/snapshot?path=$SHOTS/edits-card-$1-$WHEEL_APPEARANCE.png" >/dev/null && echo "shot $SHOTS/edits-card-$1-$WHEEL_APPEARANCE.png"; }
app_post "/scroll?agent=$AG&from_end=0" >/dev/null; sleep 0.5
shot folded

C="$(card_id 20)"
click "$C"
check "5. a click on the … row lists all twenty" has 20 'listed src/file-1.go'
listed_count() { card "$1" | sed 's/ · open.*//' | grep -o 'src/file-[0-9]*\.go +2 -1' | wc -l | tr -d ' '; }
check "5. all twenty files are listed" test "$(listed_count 20)" = 20
check "5. the row reads Show fewer, the … row is gone" has 20 ' · show fewer · open'
app_post "/scroll?agent=$AG&from_end=0" >/dev/null; sleep 0.5
shot listed
click "$C"
check "5. a click on Show fewer folds back to three" has 20 '· more "… 17 more files +34 -17"'

# Per-file diffs.
check "13. Collapse all is absent while no diff is open" has 20 'collapse_all hidden'
clickp "$C" src/file-2.go
check "9. clicking file 2 opens only file 2's diff" has 20 'open [src/file-2.go] · collapse_all shown'
clickp "$C" src/file-3.go
check "10. opening file 3 leaves file 2 open" has 20 'open [src/file-2.go,src/file-3.go]'
clickp "$C" src/file-2.go
check "11. clicking file 2 again closes only it" has 20 'open [src/file-3.go] · collapse_all shown'
clickp "$C" src/file-2.go
check "8. the header and the other diffs work while folded" has 20 'open [src/file-2.go,src/file-3.go]'
app_post "/scroll?agent=$AG&from_end=0" >/dev/null; sleep 0.7
shot two-open
C4="$(card_id 4)"
clickp "$C4" src/file-1.go
check "9. a diff in another card opens on its own" has 4 'open [src/file-1.go]'
check "9. and leaves the twenty card's two open" has 20 'open [src/file-2.go,src/file-3.go]'
click "$C4" collapse=all
check "12. Collapse all closes the card's open diffs, then hides itself" has 4 'open [] · collapse_all hidden'
check "12. and leaves another card's diffs open" has 20 'open [src/file-2.go,src/file-3.go]'

# A diff open on a file the fold hides keeps "Collapse all" showing.
click "$C" collapse=all
click "$C"
clickp "$C" src/file-9.go
click "$C"
check "Collapse all stays while only a hidden file's diff is open" has 20 'open [src/file-9.go] · collapse_all shown'
click "$C" collapse=all
check "and closes it" has 20 'open [] · collapse_all hidden'
clickp "$C" src/file-2.go
clickp "$C" src/file-3.go

# Scrolling away and back.
click "$C"
app_post "/scroll?agent=$AG&from_end=100000" >/dev/null; sleep 0.7
app_post "/scroll?agent=$AG&from_end=0" >/dev/null; sleep 0.7
check "6. the fold state survives scrolling away and back" has 20 'listed src/file-1.go'
check "6. … still listed in full" has 20 ' · show fewer · open [src/file-2.go,src/file-3.go]'
check "14. per-file open state survives scrolling away and back" has 20 'open [src/file-2.go,src/file-3.go]'
click "$C"

click "$C" collapse=all
check "12. Collapse all on the twenty card closes both and hides itself" has 20 'open [] · collapse_all hidden'
app_post "/scroll?agent=$AG&from_end=0" >/dev/null; sleep 0.7
shot collapsed

# chat.edits.shown 1.
w config set chat.edits.shown 1 --machine >/dev/null
check "7. chat.edits.shown 1 lists one file then … 19 more files" wait_st ".panes[\"$P\"].transcript_items | map(select(startswith(\"edits 20 files · listed src/file-1.go +2 -1 · more \\\"… 19 more files\")))| length == 1" 10
check "7. and a four-file card lists one, … 3 more files" has 4 '· more "… 3 more files +6 -3"'
w config set chat.edits.shown 3 --machine >/dev/null

# Dark.
WHEEL_APPEARANCE=dark
quit_app
launch_app
app_post "/focus?space=$A" >/dev/null
wait_st ".panes[\"$P\"].transcript_items | map(select(startswith(\"edits \"))) | length == 4" 20
C="$(card_id 20)"
app_post "/scroll?agent=$AG&from_end=0" >/dev/null; sleep 0.5
shot folded
clickp "$C" src/file-2.go; clickp "$C" src/file-3.go
app_post "/scroll?agent=$AG&from_end=0" >/dev/null; sleep 0.7
shot two-open
click "$C" collapse=all
shot collapsed
finish
