#!/usr/bin/env bash
# § Select and copy across the overhauled reply (wheel-e916.16): a pointer selection runs from a
# reply's first paragraph, through a quote, a code block, a file link, an image, the edited-files
# card and a later reply, and ⌘C puts the text on the pasteboard in reading order. Drives the real
# Mac app against this script's own daemon with the fake agent, which echoes the prompt as the reply
# and (on "diff") edits a file. The pointer is a stream of mouse events sent through the application
# (`POST /transcript/mouse`), so the transcript's mouse monitor and the links under the pointer see
# what a person's pointer would send. Window shots (cacheDisplay) go to $WHEEL_SHOTS.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"
THEME="${WHEEL_APPEARANCE:-light}"

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
# Inside a Wheel pane the CLI would ask the person's own wheeld which space holds this pane.
unset WHEEL_SPACE WHEEL_PANE

mkdir -p "$SCR/a/src"
printf 'one\n' >"$SCR/a/src/a.rs"
printf '# Notes\n\nA line to preview.\n' >"$SCR/a/notes.md"
python3 - "$SCR/a/shot.png" <<'EOF'
import struct, sys, zlib
w, h = 160, 90
rows = b"".join(b"\x00" + (b"\x0a\x84\xff" * (w // 2) + b"\xff\x9f\x0a" * (w // 2)) for _ in range(h))
def chunk(t, d): return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
open(sys.argv[1], "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
EOF

A="$(w space new "$SCR/a")"
P="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
w pane start "$P" agent >/dev/null

NOTES="$SCR/a/notes.md"
IMG="$SCR/a/shot.png"
FIRST="First paragraph opens the reply (diff)."
LAST="Final paragraph of the second reply."
PROMPT=$"$FIRST"$'\n\n> A quoted line that holds words\n\n```swift\nlet answer = 42\nprint(answer)\n```\n\nOpen '"$NOTES"$' for the notes.\n\n!['"shot"']('"$IMG"$')\n\nClosing paragraph of the first reply.'
w agent send "$P" "$PROMPT" >/dev/null
w agent wait "$P" >/dev/null
w agent send "$P" "$LAST" >/dev/null
w agent wait "$P" >/dev/null

launch_app
check "the app connects" wait_st '.connection == "connected"' 20
app_post "/focus?space=$A" >/dev/null
check "both replies arrive" wait_st ".panes[\"$P\"].transcript | map(select(startswith(\"agent: echo\"))) | length == 2" 20
sleep 3

AGENT_ID="$(st -r ".panes[\"$P\"].agent")"
ITEMS="$(st -r ".panes[\"$P\"].transcript_items[]")"
echo "--- transcript items:"; echo "$ITEMS"
USER1="$(awk '$1=="user"{print $2; exit}' <<<"$ITEMS")"
WORK="$(awk '$1=="work"{print $2; exit}' <<<"$ITEMS")"
R1="$(awk '$1=="agent"{print $2; exit}' <<<"$ITEMS")"
EDITS="$(awk '$1=="edits"{print $NF}' <<<"$ITEMS" | sed 's/^@//')"
U2="$(awk '$1=="user"{n=$2} END{print n}' <<<"$ITEMS")"
R2="$(awk '$1=="agent"{n=$2} END{print n}' <<<"$ITEMS")"
echo "user $USER1, work $WORK, reply $R1, edits $EDITS, user $U2, reply $R2"

shoot() { app_post "/window/snapshot?path=$SHOTS/selection-cases-$1-$THEME.png" >/dev/null; echo "     shot $SHOTS/selection-cases-$1-$THEME.png"; }
prose_of() { app_get "/transcript/drawn?agent=$AGENT_ID&row=$1" | jq -r .prose; }
# offset_of <prose> <needle>: UTF-16 offset of the needle's first character.
offset_of() { python3 -c 'import sys; t=sys.argv[1]; i=t.find(sys.argv[2]); print(-1 if i<0 else len(t[:i].encode("utf-16-le"))//2)' "$1" "$2"; }
length_of() { python3 -c 'import sys; print(len(sys.argv[1].encode("utf-16-le"))//2)' "$1"; }
slice_of() { python3 -c 'import sys; b=sys.argv[1].encode("utf-16-le"); print(b[int(sys.argv[2])*2:int(sys.argv[3])*2].decode("utf-16-le"), end="")' "$1" "$2" "$3"; }
point_of() { app_get "/transcript/point?agent=$AGENT_ID&row=$1&offset=$2"; }
# mouse <from row> <from offset> [<to row> <to offset>] [extra query]: a pointer press at the character, a drag to the other one.
mouse() {
  local fp tp q
  fp="$(point_of "$1" "$2")"
  q="x=$(jq -r .x <<<"$fp")&y=$(jq -r .y <<<"$fp")"
  if [ -n "${3:-}" ]; then tp="$(point_of "$3" "$4")"; q="$q&to_x=$(jq -r .x <<<"$tp")&to_y=$(jq -r .y <<<"$tp")"; fi
  app_post "/transcript/mouse?agent=$AGENT_ID&$q${5:+&$5}"
}

PROSE1="$(prose_of "$R1")"
PROSE2="$(prose_of "$R2")"
P1LEN="$(length_of "$PROSE1")"; P2LEN="$(length_of "$PROSE2")"
THEME="${WHEEL_APPEARANCE:-light}"

# 1. A drag in the first paragraph, then a shift-click in the last reply: the selection runs
#    through the quote, the code block, the file link, the image, the edited-files card and the
#    next message and reply, and the copy is in reading order.
F="$(offset_of "$PROSE1" "First paragraph")"
got="$(mouse "$R1" "$F" "$R1" $((F + 5)))"
eq "a drag in the first paragraph selects its first word" "$(jq -r .pasteboard <<<"$got")" "First"
got="$(mouse "$R2" "$P2LEN" "" "" "shift=1")"
CLOSING="Closing paragraph of the first reply."
EXPECT=$'First paragraph opens the reply (diff).\n\nA quoted line that holds words\n\nlet answer = 42\nprint(answer)\n\n\nOpen '"$NOTES"$' for the notes.\n\n![shot]('"$IMG"$')\n\n'"$CLOSING"$'\n\nEdited 1 file\nsrc/a.rs\n\n'"$LAST"$'\n\necho: '"$LAST"
echo "  copied: $(jq -c .pasteboard <<<"$got")"
eq "a shift-click in the last reply copies the whole run in reading order" "$(jq -r .pasteboard <<<"$got")" "$EXPECT"
eq "the selection's anchor is the first paragraph's first word" "$(jq -c '.anchor' <<<"$got")" "{\"offset\":$F,\"row\":$R1}"
eq "the selection's head is the end of the last reply" "$(jq -c '.head' <<<"$got")" "{\"offset\":$P2LEN,\"row\":$R2}"
shoot through-last
app_get "/transcript/drawn?agent=$AGENT_ID&row=$R1" | jq -r '.lines[] | "     \(.from)-\(.to) \(.text|@json)"'

# 2. ⌘C puts the same text on the pasteboard.
got="$(app_post "/transcript/key?agent=$AGENT_ID&text=c&mods=cmd")"
eq "⌘C is taken by the transcript" "$(jq -r .taken <<<"$got")" "true"
eq "⌘C copies the selection" "$(jq -r .pasteboard <<<"$got")" "$EXPECT"

# 3. An image and a file link copy as their source and their full path.
IMGOFF="$(offset_of "$PROSE1" $'￼')"
got="$(mouse "$R1" "$IMGOFF" "$R1" $((IMGOFF + 1)))"
eq "an image copies as its Markdown source" "$(jq -r .pasteboard <<<"$got")" "![shot]($IMG)"
shoot image
LINKOFF="$(offset_of "$PROSE1" "$(basename "$NOTES")")"
got="$(mouse "$R1" "$LINKOFF" "$R1" $((LINKOFF + 8)))"
eq "a file link copies as its full path" "$(jq -r .pasteboard <<<"$got")" "$NOTES"
eq "selecting the link by drag opens nothing" "$(st ".panes[\"$P\"].links.click")" "null"

# 4. A drag that starts on a file link selects text; a plain click on it opens the file in a pane.
got="$(mouse "$R1" $((LINKOFF + 2)) "$R1" $((LINKOFF + 22)))"
echo "  drag from the link: $(jq -c . <<<"$got")"
eq "a drag from inside a file link selects from that character" "$(jq -r .anchor.offset <<<"$got")" "$((LINKOFF + 2))"
eq "a drag from inside a file link opens nothing" "$(st ".panes[\"$P\"].links.click")" "null"
shoot link-drag
got="$(mouse "$R1" $((LINKOFF + 2)))"
echo "  pointer press and release on the link: $(jq -c . <<<"$got")"
check "a pointer click on a file link opens notes.md in a pane" wait_st ".panes[\"$P\"].links.click.page == \"file://$NOTES\"" 5
eq "the app log has the one click from the transcript's pointer handler" "$(grep -c "link clicked in row $R1: wheel-link://file" "$INST/WheelMac.log")" "1"
CLICKED="$(st -r ".panes[\"$P\"].links.click.pane")"
w pane close "$CLICKED" >/dev/null
check "the pane the click opened closes" wait_st "(.panes[\"$CLICKED\"] // null) == null" 10
grep "link clicked in row $R1" "$INST/WheelMac.log" | tail -1 | cut -c1-200 | sed 's/^/     /'
LP="$(point_of "$R1" $((LINKOFF + 2)))"
LURL="$(app_get "/transcript/link?agent=$AGENT_ID&x=$(jq -r .x <<<"$LP")&y=$(jq -r .y <<<"$LP")" | jq -r .url)"
echo "  link under that point: $LURL"
check "the link drawn under the pointer is the file link to notes.md" test "$(python3 -c 'import sys, urllib.parse as u; print(u.unquote(sys.argv[1]))' "$LURL")" = "wheel-link://file?path=$NOTES"
app_post "/agent/link?agent=$AGENT_ID&row=$R1&url=$(python3 -c 'import sys, urllib.parse as u; print(u.quote(sys.argv[1], safe=""))' "$LURL")" >/dev/null
check "the click action on that link opens notes.md in a new pane" wait_st ".panes[\"$P\"].links.click | .page == \"file://$NOTES\" and .pane != null and .pane != \"$CLICKED\"" 5
shoot link-click

# 5. The work line copies its label; the copy button copies the reply as Markdown.
U1LEN="$(length_of "$(app_post "/transcript/select?agent=$AGENT_ID&from_row=$USER1&from_offset=0&to_row=$USER1&to_offset=100000" | jq -r .pasteboard)")"
got="$(app_post "/transcript/select?agent=$AGENT_ID&from_row=$USER1&from_offset=$((U1LEN - 4))&to_row=$R1&to_offset=4")"
echo "  across the work line: $(jq -c .pasteboard <<<"$got")"
check "a collapsed work line copies its label" jq -e '.pasteboard | test("ply\\.\n\nWorked( for [0-9<][^\n]*)?\n\necho$")' <<<"$got"
got="$(app_post "/reply/copy?agent=$AGENT_ID&row=$R1")"
eq "the copy button copies the reply's Markdown unchanged" "$(jq -r .pasteboard <<<"$got")" "echo: $PROMPT"

# 6. The drag lands on the characters under the pointer in a list with inline code, at the
#    default chat size and at 15 pt / 1.6.
LIST=$'Steps to follow:\n\n1. Run ssh deck@steamdeck with the flag `--user` and then open the folder on your Deck to confirm it\n2. Second item with `inline code` and plain words after it\n\nClosing words.'
w agent send "$P" "$LIST" >/dev/null
w agent wait "$P" >/dev/null
check "the list reply arrives" wait_st ".panes[\"$P\"].transcript_items | map(select(startswith(\"agent \"))) | length == 3" 20
list_drag() {
  local tag="$1" row prose from to got
  row="$(st -r ".panes[\"$P\"].transcript_items | map(select(startswith(\"agent \"))) | .[-1] | split(\" \")[1]")"
  prose="$(prose_of "$row")"
  from=$(( $(offset_of "$prose" '--user') + 2 )); to=$(( $(offset_of "$prose" 'inline code') + 3 ))
  eval "Y_${tag//-/_}=$(point_of "$row" "$to" | jq -r .y)"
  got="$(mouse "$row" "$from" "$row" "$to")"
  echo "  $tag: $(jq -c . <<<"$got")"
  eq "$tag: the drag starts at the character under the pointer" "$(jq -r .anchor.offset <<<"$got")" "$from"
  eq "$tag: the drag ends at the character under the pointer" "$(jq -r .head.offset <<<"$got")" "$to"
  eq "$tag: the copy is the text between the two characters" "$(jq -r .pasteboard <<<"$got")" "$(slice_of "$prose" "$from" "$to")"
  shoot "list-$tag"
}
list_drag "size-13"
w config set --machine chat.font.size 15 >/dev/null
w config set --machine chat.line_height 1.6 >/dev/null
sleep 3
list_drag "size-15"
check "the layout moved with the chat size (item 2 at y $Y_size_13 at 13 pt, $Y_size_15 at 15 pt)" test "$Y_size_13" != "$Y_size_15"
finish
