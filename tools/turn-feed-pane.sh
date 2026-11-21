#!/bin/bash
# Draw the turn-feed pane and the gallery's WebView{artifact} row in the hidden GPUI app (never shown,
# never focused) against a throwaway wheeld running the fake agent, and save PNGs under /private/tmp/claude/wheel/shots.
# The project holds the turn-feed and gallery plugins (symlinked), a markdown post artifact "tf-sample" (heading, table,
# SVG image), and two posts from the fake agent pane: one with a Markdown body, a question, options 1A and 1B and the
# page "tf-sample", and an older one with only a title and a body. Pick 1A is pressed through the app's debug /plugin
# route; the check then reads the agent's transcript for "picked 1A". Needs jq-free python3 and curl only.
#
#   tools/turn-feed-pane.sh [path to a Wheel*.app holding wheel-gpui and libWheelGpuiBridge.dylib]
#   default: clients/gpui/target/app/Wheel Dev.app
#   needs cargo build -p turn-feed -p wheeld -p wheel-cli --bins
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP=${1:-$ROOT/clients/gpui/target/app/Wheel Dev.app}
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
FAKE=${FAKE:-$ROOT/target/debug/wheel-fake-agent}
T=/private/tmp/claude/wheel/tfpane
SHOTS=/private/tmp/claude/wheel/shots
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
PORT=$(free_port)
DEBUG=$(free_port)
PIDS=()
FAILS=0
cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null; done
  sleep 0.5
  for pid in "${PIDS[@]}"; do kill -9 "$pid" 2>/dev/null; done
}
trap cleanup EXIT
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }

for f in "$WHEELD" "$WHEEL" "$FAKE" "$ROOT/target/debug/turn-feed" "$APP/Contents/MacOS/wheel-gpui"; do
  [ -x "$f" ] || { echo "missing $f" >&2; exit 2; }
done
rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/posts" "$T/proj/.wheel/plugins" "$SHOTS"
printf '[agent.claude]\ncommand = "%s"\n' "$FAKE" > "$T/machine.toml"
ln -s "$ROOT/plugins/turn-feed" "$T/proj/.wheel/plugins/turn-feed"
ln -s "$ROOT/plugins/gallery" "$T/proj/.wheel/plugins/gallery"

TURN_FEED_DIR="$T/posts" SHELL=/bin/sh env -u WHEEL_SPACE -u WHEEL_PANE "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" \
  --machine-file "$T/machine.toml" --log-dir "$T/logs" --cli "$WHEEL" > "$T/wheeld.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { env -u WHEEL_SPACE -u WHEEL_PANE "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }
AUTH="Authorization: Bearer $(cat "$T/support/token")"
dbg() { curl -s -G -H "$AUTH" -X POST "http://127.0.0.1:$DEBUG/$1" "${@:2}"; }
plug() { dbg plugin --data-urlencode "pane=$PANE" "$@"; }
shot() { w pane shot "$PANE" "$SHOTS/$1.png" > "$T/shot-$1.out" 2>&1 && pass "snapshot $SHOTS/$1.png" || fail "snapshot $1: $(cat "$T/shot-$1.out")"; }

scroll_to() { dbg pane/scroll --data-urlencode "pane=$PANE" --data-urlencode "to=$1" > "$T/scroll-$1.json"; }

SPACE=$(w space new "$T/proj")
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
AGENT_PANE=$(w pane split "$PANE" right)
w pane start "$AGENT_PANE" agent --agent claude > "$T/start.txt" 2>&1

# A markdown post artifact: heading, a table and an SVG image beside it.
cat > "$T/flow.svg" <<'SVG'
<svg xmlns="http://www.w3.org/2000/svg" width="360" height="90" viewBox="0 0 360 90">
  <rect x="10" y="20" width="100" height="50" rx="6" fill="#dbeafe" stroke="#1d4ed8"/>
  <text x="60" y="50" font-family="sans-serif" font-size="14" text-anchor="middle">agent</text>
  <rect x="250" y="20" width="100" height="50" rx="6" fill="#dcfce7" stroke="#15803d"/>
  <text x="300" y="50" font-family="sans-serif" font-size="14" text-anchor="middle">feed</text>
  <path d="M110 45 H240" stroke="#111" stroke-width="2" fill="none"/>
  <path d="M240 45 l-10 -6 v12 z" fill="#111"/>
</svg>
SVG
cat > "$T/index.md" <<'MD'
# Store layout

| Store | Writes |
| --- | --- |
| files | atomic |
| database | locked |

![flow](flow.svg)
MD
w artifact builtin install markdown > "$T/builtin.out" 2>&1 || echo "NOTE builtin install: $(cat "$T/builtin.out")"
(cd "$T" && w artifact new tf-sample --space "$SPACE" --renderer markdown index.md flow.svg > "$T/artifact.out" 2>&1) \
  && pass "artifact tf-sample made: $(tail -1 "$T/artifact.out")" || fail "artifact new: $(cat "$T/artifact.out")"

w artifact new gallery-sample --space "$SPACE" --renderer markdown "$T/index.md" "$T/flow.svg" > "$T/artifact2.out" 2>&1 || echo "NOTE gallery-sample: $(cat "$T/artifact2.out")"
cat > "$T/post1.json" <<'JSON'
{"title": "Pick a store for the feed",
 "body": "## Two ways to keep posts\n\nBoth work; the trade is in the writes.\n\n- one file per post, written with `rename`\n- one database, written under a lock\n\n| Store | Writes |\n| --- | --- |\n| files | atomic |\n| database | locked |",
 "question": "1. Where do posts live?",
 "options": [
  {"id": "1A", "action": "One file per post", "case": "Atomic writes, nothing to migrate", "cost": "Many small files", "checked": "ran the store tests", "pick": true},
  {"id": "1B", "action": "One database", "case": "One place to query", "cost": "A lock for every write", "checked": "nothing claimed"}],
 "page": "tf-sample"}
JSON
cat > "$T/post0.json" <<'JSON'
{"title": "Earlier note", "body": "Only a title and a body: the older post."}
JSON
w plugin action "$AGENT_PANE" turn-feed.post --args-file "$T/post0.json" > "$T/post0.out" 2>&1 && sleep 1.1
w plugin action "$AGENT_PANE" turn-feed.post --args-file "$T/post1.json" > "$T/post1.out" 2>&1
grep -q '"post"' "$T/post0.out" "$T/post1.out" && pass "both posts accepted" || fail "posts: $(cat "$T/post0.out" "$T/post1.out")"

w pane kind "$PANE" plugin --plugin turn-feed --plugin-pane feed > /dev/null

env -u WHEEL_SPACE -u WHEEL_PANE WHEEL_HIDDEN=1 WHEEL_DEBUG_PORT=$DEBUG WHEEL_URL="ws://127.0.0.1:$PORT/ws" \
  WHEEL_TOKEN_FILE="$T/support/token" WHEEL_CLIENT_SPACE="$SPACE" \
  WHEEL_GPUI_BRIDGE="$APP/Contents/MacOS/libWheelGpuiBridge.dylib" \
  "$APP/Contents/MacOS/wheel-gpui" --space "$SPACE" > "$T/app.out" 2>&1 &
PIDS+=($!)

# The feed's pane is the split's first pane: give it 90% of the window's width so the cards and the gallery's columns fit.
SPLIT=$(w space ls | python3 -c 'import json,sys; print(json.load(sys.stdin)["spaces"][0]["root"]["id"])')
w pane ratio "$SPLIT" 0.9 > /dev/null 2>&1 || fail "pane ratio $SPLIT"

# q <python over d (the pane's /plugin answer), texts, comps(type), by_text(text)>: prints the value, or "no state".
q() {
  plug --data-urlencode action=state > "$T/pane.json" 2>/dev/null
  python3 - "$T/pane.json" "$1" <<'PY'
import json, sys
try: d = json.load(open(sys.argv[1]))
except Exception: print("no state"); sys.exit()
cs = [c for c in d.get("components", []) if isinstance(c, dict)]
texts = [c.get("text") for c in cs if c.get("text")]
def comps(kind): return [c for c in cs if c.get("type") == kind]
def button_with(label, nth=0):
    # The Button whose id is the id of the Text that says label, up to the Text's own suffix.
    found = []
    for t in cs:
        if t.get("type") == "Text" and t.get("text") == label:
            for b in comps("Button"):
                if t["id"].startswith(b["id"]): found.append(b)
    return found[nth] if nth < len(found) else None
print(eval(sys.argv[2]))
PY
}
wait_for() {
  local end=$((SECONDS + $1))
  while [ $SECONDS -lt $end ]; do
    [ "$(q "bool($2)")" = True ] && return 0
    sleep 0.4
  done
  return 1
}

wait_for 60 'd.get("error") is None and "Pick a store for the feed" in texts' && pass "the feed draws the post" ||
  fail "the feed: $(q '(d.get("error"), texts[:6])')"
wait_for 30 'len(comps("Grid")) >= 2 and len(comps("Image")) >= 1' && pass "the body and page tables (Grid) and the page image (Image) are in the drawn tree" ||
  fail "tree: $(q '[c["type"] for c in cs][:40]')"
sleep 4
# The feed's List scrolls itself and takes no debug scroll, so a shot shows the top of the newest card. The paged card is
# newest first: the page shows in this shot; a short card with the same options is then posted above it so its option
# blocks, Pick buttons and answer line show in the next shots.
shot feed-page
cat > "$T/post2.json" <<'JSON'
{"title": "Short card: pick a queue", "body": "A short body in **Markdown**, so the options fit above the fold.",
 "question": "2. Which queue holds the jobs?",
 "options": [
  {"id": "1A", "action": "A table in the store", "case": "One place to look", "cost": "Polling to wake a worker", "checked": "ran the queue tests", "pick": true},
  {"id": "1B", "action": "A named pipe", "case": "Wakes at once", "cost": "Jobs are lost on a crash", "checked": "nothing claimed"}]}
JSON
w plugin action "$AGENT_PANE" turn-feed.post --args-file "$T/post2.json" > "$T/post2.out" 2>&1 && pass "short card posted" || fail "short card: $(cat "$T/post2.out")"
wait_for 30 '"Short card: pick a queue" in texts' && pass "the short card is drawn" || fail "short card not drawn"
sleep 3
shot feed-before-pick

press() { plug --data-urlencode action=press --data-urlencode "component=$(q "button_with(\"Pick 1A\", $1)[\"id\"]")" > "$T/press.json"; }
press 0
wait_for 30 'any(t.startswith("Picked 1A") for t in texts)' && pass "the short card reads Picked 1A" || fail "after pick: $(q 'texts[:16]')"
wait_for 10 '(button_with("Pick 1A", 0) or {}).get("disabled") == "true" and (button_with("Pick 1B", 0) or {}).get("disabled") == "true"' && pass "the short card's Pick buttons are disabled in the tree" ||
  fail "buttons not disabled: $(q 'button_with("Pick 1A")')"
sleep 2
shot feed-after-pick
press 1
wait_for 30 'sum(1 for t in texts if t.startswith("Picked 1A")) == 2' && pass "the paged card reads Picked 1A too" || fail "second pick: $(q 'texts')"
sent() { w plugin feed "$AGENT_PANE" wheel.agent.transcript --params "{\"agent\":\"$AGENT_PANE\"}" --count 1 2> /dev/null | grep -q "picked 1A"; }
ok=1; for _ in $(seq 1 50); do sent && { ok=0; break; }; sleep 0.2; done
[ $ok -eq 0 ] && pass "the fake agent's transcript received picked 1A" || fail "the agent's transcript has no picked 1A"

# The gallery: the WebView{artifact} row, scrolled into view.
w pane kind "$PANE" plugin --plugin gallery --plugin-pane gallery > /dev/null
wait_for 40 'any(t == "Component gallery" for t in texts)' && pass "the gallery draws" || fail "gallery: $(q 'texts[:4]')"
# gallery_row <name> <scroll arg> <text the row draws>: scrolls the pane (to=top|bottom, or dy=<points> from the top), then snapshots.
gallery_row() {
  scroll_to top; sleep 1
  case "$2" in dy=*) dbg pane/scroll --data-urlencode "pane=$PANE" --data-urlencode "$2" > /dev/null ;; *) scroll_to "${2#to=}" ;; esac
  sleep 4
  [ "$(q "any(t.startswith(\"$3\") for t in texts)")" = True ] && pass "gallery row \"$3\" is in the drawn tree" || fail "gallery row \"$3\" is not drawn"
  shot "$1"
}
gallery_row gallery-webview-artifact to=bottom "artifact: an artifact"

echo "debug $DEBUG pane $PANE token $T/support/token"; [ -n "${HOLD:-}" ] && sleep "$HOLD"
echo "snapshots in $SHOTS"
[ $FAILS -eq 0 ] && echo "ALL PASS" || echo "$FAILS FAILED"
exit $FAILS
