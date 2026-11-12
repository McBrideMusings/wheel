#!/usr/bin/env bash
# A click on a link in a reply (wheel-h3z5.23). A file path opens its file in a browser pane beside
# the chat at its file:// address, with the line the link names as #L<n>; a web link opens in a
# browser pane; a path that does not exist is not a link; chat.links.files off leaves paths as plain
# text. No popover opens. /state's `links.click` names the last click's action, page and the pane now
# showing it. The click is /agent/link, the same ChatLinkResolver.click the reply's link runs.
# Drives the built Mac app against its own daemon.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS" "$SCR/alpha"
printf 'import Foundation\nlet one = 1\nlet two = 2\n' >"$SCR/alpha/Main.swift"
printf '# Title\n\nbody text\n' >"$SCR/alpha/NOTES.md"
python3 - "$SCR/alpha/shot.png" <<'EOF'
import struct, sys, zlib
w, h = 40, 30
rows = b"".join(b"\x00" + (b"\x0a\x84\xff" * w) for _ in range(h))
def chunk(t, d): return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
open(sys.argv[1], "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
EOF
export CLAUDE_CONFIG_DIR="$SCR/claude"
use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset

A="$(w space new "$SCR/alpha")"
launch_app
wait_st ".active == \"$A\"" 20
P="$(st --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
w pane start "$P" agent >/dev/null
wait_st ".panes[\"$P\"].kind == \"agent\"" 15
S="$(st --arg p "$P" '.panes[$p].agent')"
say() { curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" --data-binary "$1" "http://127.0.0.1:$DEBUG_PORT/agent?agent=$S" >/dev/null; }
wait_p() { wait_st ".panes[\"$P\"] | $1" "${2:-10}"; }

say "Files: $SCR/alpha/Main.swift, $SCR/alpha/NOTES.md, $SCR/alpha/Main.swift:2, $SCR/alpha/shot.png, $SCR/alpha/none.swift. Web: https://example.com/page"
check "four existing files and the web link are links" wait_p '([.links.linked[].markdown | scan("\\(wheel-link://file[^)]*\\)|\\(https://example.com/page\\)")] | length) == 5' 30
MD="$(st --arg p "$P" '.panes[$p].links.linked[0].markdown')"
ROW="$(st --arg p "$P" '.panes[$p].links.linked[0].row')"
eq "the missing path is not a link" "$(printf '%s' "$MD" | grep -c 'none.swift)')" "0"
eq "the popover is gone: no preview in /state" "$(st --arg p "$P" '.panes[$p].links | has("preview")')" "false"

# The address of the link whose label is $1.
addr() { printf '%s' "$MD" | grep -o "\[$1\]([^)]*)" | head -1 | sed -e 's/^[^(]*(//' -e 's/)$//'; }
# Click $1 (a link address), wait for the pane it opens; print "<action> <page> <pane kind>".
click() {
  app_post "/agent/link?agent=$S&row=$ROW&url=$(jq -rn --arg u "$1" '$u|@uri')" >/dev/null
  wait_p '.links.click.pane != null' 15
  st --arg p "$P" '.panes[$p].links.click.page'
}
pane_of() { st --arg p "$P" '.panes[$p].links.click.pane'; }
kind_of() { st --arg id "$1" '.panes[$id].kind'; }
url_of() { st --arg id "$1" '.panes[$id].url'; }

# 1. a .swift path
PAGE="$(click "$(addr 'Main.swift')")"
eq "1: a .swift path opens a browser pane at its file URL" "$PAGE" "file://$SCR/alpha/Main.swift"
N1="$(pane_of)"
eq "1: the pane is a browser" "$(kind_of "$N1")" "browser"
eq "1: its page is the file" "$(url_of "$N1")" "file://$SCR/alpha/Main.swift"
app_post "/window/snapshot?window=$(win_of "$A")&path=$SHOTS/chat-link-click-swift.png" >/dev/null
echo "     shot $SHOTS/chat-link-click-swift.png"

# 2. a .md path
PAGE="$(click "$(addr 'NOTES.md')")"
eq "2: a .md path opens at its file URL" "$PAGE" "file://$SCR/alpha/NOTES.md"
eq "2: in a browser pane" "$(kind_of "$(pane_of)")" "browser"

# 3. a path with :line
PAGE="$(click "$(addr 'Main.swift:2')")"
eq "3: a path with :2 opens at #L2" "$PAGE" "file://$SCR/alpha/Main.swift#L2"
eq "3: the pane's page carries the line" "$(url_of "$(pane_of)")" "file://$SCR/alpha/Main.swift#L2"

# 4. an image path
PAGE="$(click "$(addr 'shot.png')")"
eq "4: an image path opens at its file URL" "$PAGE" "file://$SCR/alpha/shot.png"
eq "4: in a browser pane" "$(kind_of "$(pane_of)")" "browser"

# 6. a web URL
PAGE="$(click "https://example.com/page")"
eq "6: a web link opens in a browser pane" "$PAGE" "https://example.com/page"
eq "6: whose page is the address" "$(url_of "$(pane_of)")" "https://example.com/page"

# 5. a missing path has no link, so no click: the reply's text keeps it plain
eq "5: the missing path stays plain text" "$(printf '%s' "$MD" | grep -c '\[[^]]*none.swift')" "0"
eq "5: and the text still names it" "$(printf '%s' "$MD" | grep -c 'none.swift')" "1"

# 7. chat.links.files off: paths are plain text
w config set --machine chat.links.files off >/dev/null
check "7: files off: no file link remains" wait_p '([.links.linked[0].markdown | scan("wheel-link://file")] | length) == 0' 15
eq "7: the web link is still a link" "$(st --arg p "$P" '.panes[$p].links.linked[0].markdown | contains("https://example.com/page")')" "true"
w config unset --machine chat.links.files >/dev/null
check "7: unset: file links return" wait_p '([.links.linked[0].markdown | scan("wheel-link://file")] | length) == 4' 15

# The setting no longer takes preview.
expect_exit_out 6 'preview' "chat.links.files preview is refused" w config set --machine chat.links.files preview
finish
