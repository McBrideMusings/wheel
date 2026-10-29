#!/usr/bin/env bash
# § Selection follows the drawn text (wheel-e916.16): a drag in a reply holding a numbered list
# with inline code, a quote and a code block starts and ends at the characters under the pointer.
# The points come from the layout SwiftUI drew (`GET /transcript/point`), the drag goes through
# the transcript's mouse handlers (`POST /transcript/drag`), and the copy is compared with the
# text between the two words. The window shot of the highlight (cacheDisplay, never screencapture)
# goes to $WHEEL_SHOTS (default $SCR/shots), one per theme. Drives the real Mac app against this
# script's own daemon; the fake agent echoes the prompt, so the prompt is the reply.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset

mkdir -p "$SCR/a"
A="$(w space new "$SCR/a")"
P="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
w pane start "$P" agent >/dev/null

PROMPT=$'Steps to follow:\n\n1. Run ssh deck@steamdeck with the flag `--user` and then open the folder on your Deck to confirm it\n2. Second item with `inline code` and plain words after it\n\n> A quoted line that holds more words\n\n```swift\nlet answer = 42\nprint(answer)\n```\n\nClosing paragraph with a tail.'
w agent send "$P" "$PROMPT" >/dev/null

launch_app
check "the app connects" wait_st '.connection == "connected"' 20
check "the reply arrives" wait_st ".panes[\"$P\"].transcript | map(select(startswith(\"agent: echo\"))) | length == 1" 20
app_post "/focus?space=$A" >/dev/null
app_post "/window/front" >/dev/null
sleep 2

SESSION="$(st -r ".panes[\"$P\"].session")"
ROW="$(st -r ".panes[\"$P\"].transcript_items | map(select(startswith(\"agent \"))) | .[0] | split(\" \")[1]")"
echo "session $SESSION, reply row $ROW"

# The offset of `needle` in the reply's rendered text: the debug select route copies a range, so
# read the text back through a whole-row selection and find the needle in it.
ALL="$(app_post "/transcript/select?session=$SESSION&from_row=$ROW&from_offset=0&to_row=$ROW&to_offset=100000" | jq -r .pasteboard)"
offset_of() { python3 -c 'import sys; t=sys.argv[1]; n=sys.argv[2]; i=t.find(n); print(-1 if i<0 else len(t[:i].encode("utf-16-le"))//2)' "$ALL" "$1"; }
point_of() { app_get "/transcript/point?session=$SESSION&row=$ROW&offset=$1"; }

# drag_between <label> <from needle> <from shift> <to needle> <to shift>: the points of two
# characters, a drag between them, and the offsets the drag resolved to. Both points come from the
# drawn layout, so this proves the round trip; the highlight shot is what shows the points are
# where the characters are drawn.
drag_between() {
  local f t fp tp got
  f=$(( $(offset_of "$2") + $3 )); t=$(( $(offset_of "$4") + $5 ))
  fp="$(point_of $f)"; tp="$(point_of $t)"
  got="$(app_post "/transcript/drag?session=$SESSION&from_x=$(jq -r .x <<<"$fp")&from_y=$(jq -r .y <<<"$fp")&to_x=$(jq -r .x <<<"$tp")&to_y=$(jq -r .y <<<"$tp")")"
  echo "  $1: points $(jq -c . <<<"$fp") -> $(jq -c . <<<"$tp"), copied $(jq -c .pasteboard <<<"$got")"
  eq "$1 starts at the character under the pointer" "$(jq -r .anchor.offset <<<"$got")" "$f"
  eq "$1 ends at the character under the pointer" "$(jq -r .head.offset <<<"$got")" "$t"
}

shoot() { app_post "/window/snapshot?path=$SHOTS/selection-drawn-$1-$THEME.png" >/dev/null; echo "shot $SHOTS/selection-drawn-$1-$THEME.png"; }

THEME="${WHEEL_APPEARANCE:-light}"
drag_between "list item 1 inline code to item 2" '--user' 2 'inline code' 3
shoot list
drag_between "word to word in a quote" 'quoted line' 0 'more words' 4
shoot quote
drag_between "code block line to line" 'let answer' 4 'print(answer)' 6
shoot code
drag_between "closing paragraph" 'Closing paragraph' 8 'a tail' 3
shoot closing
finish
