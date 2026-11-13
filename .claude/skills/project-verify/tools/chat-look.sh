#!/usr/bin/env bash
# § Chat reading layout (wheel-h3z5.6): the chat's defaults are Codex's reading layout, 14 pt text
# at a 1.625 line height with 14 pt between paragraphs, code inside a sentence in a rounded box, a
# code block with a hairline border. `wheel config ls` lists each key with its default, /state's
# `appearance` reports what the pane resolved, and `/transcript/drawn` gives the lines SwiftUI drew
# for one reply (paragraphs, inline code, a list, a fenced block), from which the line pitch and the
# paragraph gap are measured. A window shot (cacheDisplay) of the reply goes to $WHEEL_SHOTS
# (default $SCR/shots), light or dark by WHEEL_APPEARANCE.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
THEME="${WHEEL_APPEARANCE:-light}"
mkdir -p "$SHOTS" "$SCR/a"
use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
unset WHEEL_SPACE WHEEL_PANE

# 1. Every key the layout reads, with its default, from the daemon's registry.
CONF="$(w config ls --json)"
def() { jq -r --arg k "$1" '[.[] | select(.key == $k)][0].default | tostring' <<<"$CONF"; }
for kv in chat.font.size=14 chat.line_height=1.625 chat.paragraph_spacing=14 code.inline=box \
          code.block.border=true code.block.spacing=18 chat.list.spacing=0 chat.list.bullet=inset chat.message.style=wash; do
  eq "config ls: ${kv%%=*} is ${kv#*=}" "$(def "${kv%%=*}")" "${kv#*=}"
done

A="$(w space new "$SCR/a")"
P="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
w pane start "$P" agent >/dev/null
P1="The first paragraph is long enough to wrap onto a second line in a pane of ordinary width, so the pitch between its lines can be measured from the layout SwiftUI drew for it."
P2="A second paragraph with \`inline code\` in the middle of a sentence and \`chat.font.size\` near its end."
PROMPT="$P1"$'\n\n'"$P2"$'\n\n- one item in a list\n- a second item\n\n```swift\nlet answer = 42\nprint(answer)\n```\n\nThe closing paragraph.'
w agent send "$P" "$PROMPT" >/dev/null
w agent wait "$P" >/dev/null

launch_app
check "the app connects" wait_st '.connection == "connected"' 20
app_post "/focus?space=$A" >/dev/null
check "the reply arrives" wait_st ".panes[\"$P\"].transcript | map(select(startswith(\"agent: echo\"))) | length == 1" 20
sleep 2

# 2. What the pane resolved.
look() { st --arg p "$P" ".panes[\$p].appearance.$1 | tostring"; }
for kv in size=14 line_height=1.625 paragraph_spacing=14 inline_code=box code_block_border=true code_block_spacing=18 list_spacing=0 list_bullet=inset message_style=wash; do
  eq "appearance: ${kv%%=*} is ${kv#*=}" "$(look "${kv%%=*}")" "${kv#*=}"
done

# 3. The lines SwiftUI drew.
AGENT="$(st -r ".panes[\"$P\"].agent")"
R="$(st -r ".panes[\"$P\"].transcript_items[]" | awk '$1=="agent"{print $2; exit}')"
DRAWN="$(app_get "/transcript/drawn?agent=$AGENT&row=$R")"
echo "--- drawn lines (y, height, text):"
jq -r '.lines[] | "     \(.y|floor) \(.height*100|floor/100) \(.text[0:60]|@json)"' <<<"$DRAWN"
# Each line's text is its whole block, so a block's lines are the ones whose text holds its words.
# Codex's stylesheet sets every line in a 22.75 pt box (14 px x 1.625) and puts 14 px between
# paragraphs, so a paragraph's first line sits one pitch plus 14 below the last line above it, and
# list items with no spacing sit one pitch apart.
MEASURE="$(python3 - "$DRAWN" "$P1" "$P2" <<'EOF'
import json, sys
d = json.loads(sys.argv[1]); p1, p2 = sys.argv[2], sys.argv[3]
lines = sorted(d["lines"], key=lambda l: l["y"])
of = lambda words: [l for l in lines if words in l["text"]]
a, b = of(p1[:30]), of(p2[:20])
i1, i2 = of("one item in a list")[0], of("a second item")[0]
assert i1 is not i2, "both list items are one drawn line"
pitch = a[1]["y"] - a[0]["y"]
gap = b[0]["y"] - a[-1]["y"] - pitch
items = i2["y"] - i1["y"]
print(f"{pitch:.2f} {gap:.2f} {items:.2f} {len(a)}")
EOF
)"
[ -n "$MEASURE" ] || fail "the drawn lines do not split into the two paragraphs and two list items the measure needs" "$(jq -c '[.lines[] | {y, text: .text[0:30]}]' <<<"$DRAWN")"
read -r PITCH GAP ITEMS N1 <<<"$MEASURE"
echo "     first paragraph: $N1 lines, pitch $PITCH pt; gap between paragraphs $GAP pt; list items $ITEMS pt apart"
within() { python3 -c 'import sys; print("yes" if sys.argv[1] and abs(float(sys.argv[1]) - float(sys.argv[2])) <= 1 else "no")' "$1" "$2"; }
eq "line pitch $PITCH pt is within 1 pt of Codex's 22.75 (14 x 1.625)" "$(within "$PITCH" 22.75)" yes
eq "paragraph gap $GAP pt (beyond one pitch) is within 1 pt of Codex's 14" "$(within "$GAP" 14)" yes
eq "list items $ITEMS pt apart are within 1 pt of Codex's 22.75" "$(within "$ITEMS" 22.75)" yes

app_post "/scroll?agent=$AGENT&from_end=0" >/dev/null; sleep 0.7
OUT="$SHOTS/chat-look-$THEME.png"
app_post "/window/snapshot?window=$(win_of "$A")&path=$OUT" >/dev/null
[ -s "$OUT" ] && echo "     shot $OUT"
finish
