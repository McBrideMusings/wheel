#!/usr/bin/env bash
# Images in a reply (wheel-e916.10.2). A Markdown image or a bare image path draws in the agent
# pane, the daemon's `file_image` sends the bytes, `/state` lists each image block with its loaded
# size, and chat.images.inline false leaves a link. Drives the built Mac app against its own daemon.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
mkdir -p "$SCR/alpha"

# A 400x300 PNG of two flat colours, and a file that is not an image.
python3 - "$SCR/alpha/shot.png" <<'EOF'
import struct, sys, zlib
w, h = 400, 300
rows = b"".join(b"\x00" + (b"\x0a\x84\xff" * (w // 2) + b"\xff\x9f\x0a" * (w // 2)) for _ in range(h))
def chunk(t, d): return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
open(sys.argv[1], "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
EOF
echo hello >"$SCR/alpha/notes.txt"

# The daemon side: the bytes arrive whole, and a text file is refused.
w file image "$SCR/alpha/shot.png" --out "$SCR/fetched.png" >/dev/null
eq "file image sends the PNG's bytes" "$(cmp -s "$SCR/alpha/shot.png" "$SCR/fetched.png" && echo same)" "same"
eq "file image resolves a relative path against --cwd" "$(w file image shot.png --cwd "$SCR/alpha" --json | jq -r '.mime')" "image/png"
expect_exit 6 "file image refuses a text file" w file image "$SCR/alpha/notes.txt"
expect_exit 6 "file image reports a missing file" w file image "$SCR/alpha/none.png"

A="$(w space new "$SCR/alpha")"
launch_app
wait_st --arg s "$A" '.active == $s' 20
P="$(st --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
app_post "/key?keys=1" >/dev/null
wait_st --arg p "$P" '.panes[$p].kind == "agent"' 15
S="$(st --arg p "$P" '.panes[$p].session')"
say() { curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" --data-binary "$1" "http://127.0.0.1:$DEBUG_PORT/agent?session=$S" >/dev/null; }

say "look: ![shot]($SCR/alpha/shot.png)
$SCR/alpha/shot.png
![gone]($SCR/alpha/none.png)"
wait_st --arg p "$P" '(.panes[$p].images | length) >= 2' 20
img() { st --arg p "$P" --arg src "$1" "[.panes[\$p].images[] | select(.source == \$src)][0] | $2"; }
eq "a Markdown image loads at its pixel size" "$(img "$SCR/alpha/shot.png" '"\(.loaded) \(.width)x\(.height)"')" "true 400x300"
eq "its bytes are the file's" "$(img "$SCR/alpha/shot.png" '.bytes')" "$(stat -f %z "$SCR/alpha/shot.png")"
eq "a missing file is listed as not loaded" "$(img "$SCR/alpha/none.png" '.loaded')" "false"

# A click on the image opens it at full size.
eq "no full-size image before a click" "$(st --arg p "$P" '.panes[$p].image_full')" "null"
IDX="$(st --arg p "$P" --arg src "$SCR/alpha/shot.png" '[.panes[$p].images | to_entries[] | select(.value.source == $src)][0].key')"
app_post "/image/open?session=$S&index=$IDX" >/dev/null
wait_st --arg p "$P" '.panes[$p].image_full != null' 5
eq "a click opens the file at full size" "$(st --arg p "$P" '.panes[$p].image_full | "\(.bytes) \(.path | endswith("/alpha/shot.png"))"')" "$(stat -f %z "$SCR/alpha/shot.png") true"

# The picture is on screen: a snapshot of the window, kept for a look.
SHOT="${WHEEL_SHOTS:-$SCR}/chat-images-${WHEEL_APPEARANCE:-light}.png"
mkdir -p "$(dirname "$SHOT")"
app_post "/window/snapshot?window=$(win_of "$A")&path=$SHOT" >/dev/null
check "the snapshot of the pane with the image was written" test -s "$SHOT"
echo "     shot $SHOT"

# Images off: the same reply draws no image block.
w config set --machine chat.images.inline false >/dev/null
say "again: ![shot2]($SCR/alpha/shot.png)"
sleep 2
eq "inline off lists no new image" "$(st --arg p "$P" '[.panes[$p].images[] | select(.source == "'"$SCR"'/alpha/shot.png")] | length')" "1"
w config unset --machine chat.images.inline >/dev/null
finish
