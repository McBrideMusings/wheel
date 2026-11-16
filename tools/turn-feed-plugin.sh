#!/bin/bash
# Check the turn-feed plugin (plugins/turn-feed) end to end against a throwaway wheeld running the
# fake agent: an agent pane posts a card with `turn-feed.post` (granted by the manifest's [agent]
# table), the feed pane reads it through `turn-feed.posts`, answers it with `turn-feed.answer` and
# sends the answer text to the agent with `wheel.agent.send`. Panes that are not agents are refused.
# The backend writes its posts to a temp folder (TURN_FEED_DIR). Prints PASS or FAIL per check.
# Uses no GUI, focus, mouse or keyboard. Everything it starts is killed by pid at the end.
#
#   tools/turn-feed-plugin.sh   # needs cargo build -p turn-feed -p wheeld -p wheel-cli --bins
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
FAKE=${FAKE:-$ROOT/target/debug/wheel-fake-agent}
BACKEND=$ROOT/target/debug/turn-feed
T=/private/tmp/claude/wheel/tfp
PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
FAILS=0
PIDS=()

pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }

cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null; done
  sleep 0.5
  for pid in "${PIDS[@]}"; do kill -9 "$pid" 2>/dev/null; done
  [ -n "${KEEP_T:-}" ] || rm -rf "$T"
}
trap cleanup EXIT

for f in "$WHEELD" "$WHEEL" "$FAKE" "$BACKEND"; do
  [ -x "$f" ] || { echo "missing $f: run cargo build -p turn-feed -p wheeld -p wheel-cli --bins" >&2; exit 2; }
done

rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/posts" "$T/proj/.wheel/plugins"
printf '[agent.claude]\ncommand = "%s"\n' "$FAKE" > "$T/machine.toml"
# The package's backend command is ../../target/debug/turn-feed, run in the package's physical
# folder (the symlink's target), so it resolves to this checkout's build.
ln -s "$ROOT/plugins/turn-feed" "$T/proj/.wheel/plugins/turn-feed"

TURN_FEED_DIR="$T/posts" SHELL=/bin/sh "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" \
  --machine-file "$T/machine.toml" --log-dir "$T/logs" --cli "$WHEEL" > "$T/wheeld.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }

poll() {
  local n=$(( $1 * 10 ))
  shift
  for _ in $(seq 1 "$n"); do
    "$@" > /dev/null 2>&1 && return 0
    sleep 0.1
  done
  return 1
}
# Succeeds when the python expression $2 (over `v`, the feed's folded value) holds for the feed lines in file $1.
folds_to() {
  python3 - "$1" "$2" <<'PYEOF'
import json, sys
v = None
for line in open(sys.argv[1]):
    line = line.strip()
    if not line.startswith("{"):
        continue
    row = json.loads(line)
    if "patch" in row:
        for op in row["patch"]:
            _, field, at = op["path"].split("/")
            if op["op"] == "add" and at == "-":
                v[field].append(op["value"])
            elif op["op"] == "replace":
                v[field][int(at)] = op["value"]
            elif op["op"] == "remove":
                del v[field][int(at)]
            else:
                raise SystemExit(f"unexpected op {op}")
    else:
        v = row.get("value")
sys.exit(0 if v is not None and eval(sys.argv[2]) else 1)
PYEOF
}

SPACE=$(w space new "$T/proj")
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
w pane kind "$PANE" plugin --plugin turn-feed --plugin-pane feed > /dev/null
AGENT_PANE=$(w pane split "$PANE" right)
w pane start "$AGENT_PANE" agent --agent claude > "$T/start.txt" 2>&1
TERM_PANE=$(w pane split "$PANE" down)
w pane start "$TERM_PANE" terminal > /dev/null 2>&1
OTHER=$(w pane split "$AGENT_PANE" down)
w pane kind "$OTHER" plugin --plugin turn-feed --plugin-pane feed > /dev/null
echo "space $SPACE: feed $PANE, agent $AGENT_PANE, terminal $TERM_PANE, plugin pane $OTHER"
DIR="$T/proj"

cat > "$T/good.json" <<'JSON'
{"title": "Pick a store", "body": "Two ways to keep the posts.", "question": "Which one?",
 "options": [
  {"id": "1A", "action": "One file per post", "case": "Atomic writes", "cost": "Many small files", "checked": "ran the store tests", "pick": true},
  {"id": "1B", "action": "One database", "case": "One place", "cost": "A lock for every write", "checked": "nothing claimed"}]}
JSON
sed 's/"cost": "A lock for every write", //' "$T/good.json" > "$T/nocost.json"

# 1 the package is listed with its agent grant.
w plugin ls --space "$SPACE" --json > "$T/ls.json" 2> "$T/ls.err"
python3 - "$T/ls.json" <<'PYEOF' && pass "wheel plugin ls lists turn-feed with agent_actions [post]" || fail "wheel plugin ls lists turn-feed with agent_actions [post]: $(head -c 600 "$T/ls.json" "$T/ls.err")"
import json, sys
data = json.load(open(sys.argv[1]))
def walk(x):
    if isinstance(x, dict):
        yield x
        for v in x.values():
            yield from walk(v)
    elif isinstance(x, list):
        for v in x:
            yield from walk(v)
ok = any(d.get("id") == "turn-feed" and d.get("agent_actions") == ["post"] for d in walk(data))
sys.exit(0 if ok else 1)
PYEOF

# 2 the agent pane posts.
w plugin action "$AGENT_PANE" turn-feed.post --args-file "$T/good.json" > "$T/post.json" 2> "$T/post.err"
ID=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["post"])' "$T/post.json" 2>/dev/null)
[ -n "$ID" ] && pass "an agent pane's turn-feed.post prints {\"post\": \"$ID\"}" \
  || fail "an agent pane's turn-feed.post: $(cat "$T/post.json" "$T/post.err")"

# 3 a post missing an option's cost.
w plugin action "$AGENT_PANE" turn-feed.post --args-file "$T/nocost.json" > "$T/nocost.out" 2> "$T/nocost.err"
RC=$?
[ "$RC" -ne 0 ] && grep -q "option 1B has no cost" "$T/nocost.err" "$T/nocost.out" \
  && pass "a post missing option 1B's cost is refused: option 1B has no cost" \
  || fail "a post missing option 1B's cost is refused (exit $RC): $(cat "$T/nocost.out" "$T/nocost.err")"

# 4 a plugin pane is not granted post.
w plugin action "$OTHER" turn-feed.post --args-file "$T/good.json" > "$T/plug.out" 2> "$T/plug.err"
RC=$?
[ "$RC" -ne 0 ] && grep -q 'may not invoke turn-feed.post' "$T/plug.err" \
  && pass "a plugin pane's turn-feed.post is refused ($(head -c 200 "$T/plug.err" | tr '\n' ' '))" \
  || fail "a plugin pane's turn-feed.post is refused naming the grant (exit $RC): $(cat "$T/plug.out" "$T/plug.err")"

# 5 a terminal pane is refused.
w plugin action "$TERM_PANE" turn-feed.post --args-file "$T/good.json" > "$T/term.out" 2> "$T/term.err"
RC=$?
[ "$RC" -ne 0 ] && [ -s "$T/term.err" ] && ! grep -q '"post"' "$T/term.out" \
  && pass "a terminal pane's turn-feed.post is refused ($(head -c 200 "$T/term.err" | tr '\n' ' '))" \
  || fail "a terminal pane's turn-feed.post is refused (exit $RC): $(cat "$T/term.out" "$T/term.err")"

# 6 the feed shows the post.
P="{\"dir\":\"$DIR\"}"
# The first value of a feed on a backend that is still starting is null, so read until one holds.
# A feed reader that is still waiting for the backend sends null first, then the value.
read_posts() {
  w plugin feed "$PANE" turn-feed.posts --params "$P" --count 200 > "$1" 2> "$2" &
  local pid=$!
  PIDS+=($pid)
  poll 5 folds_to "$1" "$3"
  local rc=$?
  kill "$pid" 2> /dev/null
  return $rc
}
poll 10 read_posts "$T/feed.jsonl" "$T/feed.err" "v is not None and len(v['posts']) == 1"
folds_to "$T/feed.jsonl" "len(v['posts']) == 1 and v['posts'][0]['post'] == '$ID' and v['posts'][0]['agent_pane'] == '$AGENT_PANE' and v['posts'][0]['dir'] == '$DIR'" \
  && pass "the posts feed shows the post with agent_pane $AGENT_PANE and dir $DIR" \
  || fail "the posts feed shows the post: $(cat "$T/feed.jsonl" "$T/feed.err")"

# 7 answer, then send the text to the agent.
w plugin action "$PANE" turn-feed.answer --args "{\"post\":\"$ID\",\"pick\":\"1A\"}" > "$T/answer.json" 2> "$T/answer.err"
TEXT=$(python3 -c '
import json, sys
r = json.load(open(sys.argv[1]))
if r.get("agent_pane") == sys.argv[2] and "picked 1A" in r.get("text", ""):
    print(r["text"])' "$T/answer.json" "$AGENT_PANE" 2>/dev/null)
[ -n "$TEXT" ] && pass "turn-feed.answer returns agent_pane and text: $TEXT" \
  || fail "turn-feed.answer returns agent_pane and text containing picked 1A: $(cat "$T/answer.json" "$T/answer.err")"
python3 - "$AGENT_PANE" "$TEXT" > "$T/send-args.json" <<'PYEOF'
import json, sys
print(json.dumps({"agent": sys.argv[1], "text": sys.argv[2]}))
PYEOF
w plugin action "$PANE" wheel.agent.send --args-file "$T/send-args.json" > "$T/send.json" 2> "$T/send.err"
python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); sys.exit(0 if r["turn"] >= 1 and r["agent"] else 1)' "$T/send.json" 2>/dev/null \
  && pass "wheel.agent.send of the answer text succeeds" \
  || fail "wheel.agent.send of the answer text: $(cat "$T/send.json" "$T/send.err")"
# The feed pane's reads do not include wheel.agent.transcript, so the agent pane reads its own.
sent() { w plugin feed "$AGENT_PANE" wheel.agent.transcript --params "{\"agent\":\"$AGENT_PANE\"}" --count 1 2> /dev/null | grep -q "picked 1A"; }
poll 10 sent && pass "the agent's transcript contains the answer text" \
  || fail "the agent's transcript contains the answer text"

# 8 a second answer is refused.
w plugin action "$PANE" turn-feed.answer --args "{\"post\":\"$ID\",\"pick\":\"1B\"}" > "$T/answer2.out" 2> "$T/answer2.err"
RC=$?
[ "$RC" -ne 0 ] && grep -q "already answered" "$T/answer2.err" \
  && pass "a second answer is refused: already answered" \
  || fail "a second answer is refused (exit $RC): $(cat "$T/answer2.out" "$T/answer2.err")"

# 9 replace by id.
python3 - "$T/good.json" "$ID" > "$T/replace.json" <<'PYEOF'
import json, sys
d = json.load(open(sys.argv[1]))
d["title"] = "Pick a store, revised"
d["id"] = sys.argv[2]
print(json.dumps(d))
PYEOF
w plugin action "$AGENT_PANE" turn-feed.post --args-file "$T/replace.json" > "$T/replace.out" 2> "$T/replace.err"
grep -q "\"post\": *\"$ID\"" "$T/replace.out" && pass "posting with id $ID from the same agent pane returns the same id" \
  || fail "posting with id replaces: $(cat "$T/replace.out" "$T/replace.err")"
poll 10 read_posts "$T/feed2.jsonl" "$T/feed2.err" "v is not None and v['posts'][0]['title'] == 'Pick a store, revised'"
folds_to "$T/feed2.jsonl" "len(v['posts']) == 1 and v['posts'][0]['post'] == '$ID' and v['posts'][0]['title'] == 'Pick a store, revised'" \
  && pass "the feed holds one post with the new title" \
  || fail "the feed holds one post with the new title: $(cat "$T/feed2.jsonl" "$T/feed2.err")"

echo "$FAILS failed"
[ "$FAILS" -eq 0 ]
