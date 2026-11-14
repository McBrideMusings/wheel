#!/bin/bash
# Check wheeld's agent feeds and actions for plugin panes (wheel-pmf2.6.1) against a throwaway
# wheeld running the fake agent (target/debug/wheel-fake-agent): a plugin pane reads an agent's
# `wheel.agent.status`, `wheel.agent.transcript` and `wheel.agent.requests`, sends it a message
# with `wheel.agent.send` and answers its permission request with `wheel.agent.answer`, through
# `wheel plugin feed` and `wheel plugin action` as a plugin pane does. Prints PASS or FAIL per
# check. Uses no GUI, focus, mouse or keyboard. Everything it starts is killed by pid at the end.
#
#   tools/plugin-agent-feeds.sh        # needs cargo build -p wheeld -p wheel-cli --bins
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
FAKE=${FAKE:-$ROOT/target/debug/wheel-fake-agent}
T=/private/tmp/claude/wheel/paf
PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
FAILS=0
PIDS=()

pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }

cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null; done
  sleep 0.5
  for pid in "${PIDS[@]}"; do kill -9 "$pid" 2>/dev/null; done
}
trap cleanup EXIT

for f in "$WHEELD" "$WHEEL" "$FAKE"; do
  [ -x "$f" ] || { echo "missing $f: run cargo build -p wheeld -p wheel-cli --bins" >&2; exit 2; }
done

rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/proj/.wheel/plugins/watcher" "$T/proj/.wheel/plugins/narrow"
printf '[agent.claude]\ncommand = "%s"\n' "$FAKE" > "$T/machine.toml"
# watcher: may read and run every agent feed and action. narrow: may read the status only.
printf '[plugin]\nid = "watcher"\nversion = "0.1.0"\n\n[[pane]]\nid = "main"\ntitle = "Watcher"\nscript = "main.luau"\nreads = ["wheel.agent.*"]\nactions = ["wheel.agent.*"]\n' \
  > "$T/proj/.wheel/plugins/watcher/wheel-plugin.toml"
printf '[plugin]\nid = "narrow"\nversion = "0.1.0"\n\n[[pane]]\nid = "main"\ntitle = "Narrow"\nscript = "main.luau"\nreads = ["wheel.agent.status"]\n' \
  > "$T/proj/.wheel/plugins/narrow/wheel-plugin.toml"
echo 'return {}' > "$T/proj/.wheel/plugins/watcher/main.luau"
echo 'return {}' > "$T/proj/.wheel/plugins/narrow/main.luau"

SHELL=/bin/sh "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
  --log-dir "$T/logs" --cli "$WHEEL" > "$T/wheeld.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }

# Runs a command every 0.1 s for up to $1 seconds: succeeds as soon as the command does.
poll() {
  local n=$(( $1 * 10 ))
  shift
  for _ in $(seq 1 "$n"); do
    "$@" > /dev/null 2>&1 && return 0
    sleep 0.1
  done
  return 1
}
# Succeeds when the feed lines in file $1, folded the way a pane folds them (each `value`
# replaces, each `patch` applies: add at the end, replace, remove), give a value for which the Python expression $2 (over `v`) holds.
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
# Succeeds when file $1 holds a `patch` line (an appended event, not a whole value).
has_patch() { grep -q '"patch":' "$1"; }

SPACE=$(w space new "$T/proj")
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
w pane kind "$PANE" plugin --plugin watcher --plugin-pane main > /dev/null
AGENT_PANE=$(w pane split "$PANE" right)
w pane start "$AGENT_PANE" agent --agent claude > "$T/start.txt" 2>&1
NARROW=$(w pane split "$PANE" down)
w pane kind "$NARROW" plugin --plugin narrow --plugin-pane main > /dev/null
echo "space $SPACE: watcher $PANE, agent $AGENT_PANE, narrow $NARROW"
P="{\"agent\":\"$AGENT_PANE\"}"

# 1 the status feed names the agent and its state.
w plugin feed "$PANE" wheel.agent.status --params "$P" --count 1 > "$T/status.jsonl" 2> "$T/status.err"
folds_to "$T/status.jsonl" "v['state']['phase'] == 'ready' and v['agent']" \
  && pass "wheel.agent.status reads the agent's name and ready state" \
  || fail "wheel.agent.status reads the agent's state: $(cat "$T/status.jsonl" "$T/status.err")"

# 2 a reader of the transcript and one of the status, then a message sent through the action.
w plugin feed "$PANE" wheel.agent.transcript --params "$P" > "$T/transcript.jsonl" 2> "$T/transcript.err" &
PIDS+=($!)
w plugin feed "$PANE" wheel.agent.status --params "$P" > "$T/status2.jsonl" 2> "$T/status2.err" &
PIDS+=($!)
poll 5 test -s "$T/transcript.jsonl"
folds_to "$T/transcript.jsonl" "v['events'] == [] or all(e['kind'] != 'user' for e in v['events'])" \
  && pass "the transcript starts with no message" \
  || fail "the transcript starts with no message: $(head -c 400 "$T/transcript.jsonl")"
poll 5 test -s "$T/status2.jsonl"
w plugin action "$PANE" wheel.agent.send --args "{\"agent\":\"$AGENT_PANE\",\"text\":\"hello feeds\"}" > "$T/send.json" 2> "$T/send.err"
python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); sys.exit(0 if r["turn"] >= 1 and r["agent"] else 1)' "$T/send.json" 2>/dev/null \
  && pass "wheel.agent.send returns the agent and the queued turn" \
  || fail "wheel.agent.send returns the turn: $(cat "$T/send.json" "$T/send.err")"
poll 10 folds_to "$T/transcript.jsonl" "any(e['kind'] == 'user' and e['text'] == 'hello feeds' for e in v['events']) and 'echo: hello feeds' in ''.join(e.get('text', '') for e in v['events'] if e['kind'] == 'text')" \
  && pass "the transcript feed shows the sent message and the agent's reply" \
  || fail "the transcript feed shows the message and the reply: $(tail -c 600 "$T/transcript.jsonl")"
has_patch "$T/transcript.jsonl" && pass "new turns reach the transcript reader as patches" \
  || fail "new turns arrive as patches: $(tail -c 400 "$T/transcript.jsonl")"
poll 10 grep -q '"phase":"busy"' "$T/status2.jsonl" && poll 10 bash -c "tail -1 '$T/status2.jsonl' | grep -q '\"phase\":\"ready\"'" \
  && pass "the status feed goes busy for the turn and ready after it" \
  || fail "the status feed goes busy then ready: $(cat "$T/status2.jsonl")"

# 3 a permission request appears in the requests feed and leaves it once answered.
w plugin feed "$PANE" wheel.agent.requests --params "$P" > "$T/requests.jsonl" 2> "$T/requests.err" &
PIDS+=($!)
poll 5 test -s "$T/requests.jsonl"
folds_to "$T/requests.jsonl" "v['requests'] == []" && pass "the requests feed starts empty" \
  || fail "the requests feed starts empty: $(cat "$T/requests.jsonl" "$T/requests.err")"
w plugin action "$PANE" wheel.agent.send --args "{\"agent\":\"$AGENT_PANE\",\"text\":\"permission\"}" > /dev/null 2> "$T/send2.err"
poll 10 folds_to "$T/requests.jsonl" "len(v['requests']) == 1 and v['requests'][0]['options']" \
  && pass "a permission request appears in the requests feed with its options" \
  || fail "a permission request appears in the requests feed: $(cat "$T/requests.jsonl")"
REQ=$(python3 - "$T/requests.jsonl" <<'PYEOF'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip().startswith("{")]
reqs = [r["value"]["requests"] for r in rows if r.get("value") and r["value"]["requests"]]
print(reqs[-1][0]["id"] if reqs else "")
PYEOF
)
w plugin action "$PANE" wheel.agent.answer --args "{\"request\":\"$REQ\",\"choice\":\"allow\"}" > "$T/answer.json" 2> "$T/answer.err"
grep -q "\"request\":\"$REQ\"" "$T/answer.json" && pass "wheel.agent.answer answers the request" \
  || fail "wheel.agent.answer answers the request: $(cat "$T/answer.json" "$T/answer.err")"
poll 10 bash -c "tail -1 '$T/requests.jsonl' | grep -q '\"requests\":\[\]'" \
  && pass "the answered request leaves the requests feed" \
  || fail "the answered request leaves the feed: $(cat "$T/requests.jsonl")"
poll 10 folds_to "$T/transcript.jsonl" "'permission: allow' in ''.join(e.get('text', '') for e in v['events'] if e['kind'] == 'text')" \
  && pass "the agent goes on with the answer it was given" \
  || fail "the agent goes on with the answer: $(tail -c 600 "$T/transcript.jsonl")"
w plugin action "$PANE" wheel.agent.answer --args "{\"request\":\"$REQ\",\"choice\":\"allow\"}" > /dev/null 2> "$T/answer2.err"
grep -q "no waiting permission $REQ" "$T/answer2.err" && pass "a second answer to the same request is refused" \
  || fail "a second answer is refused: $(cat "$T/answer2.err")"

# 4 refusals: the manifest's reads and actions, missing params and args, names wheeld lacks.
w plugin feed "$NARROW" wheel.agent.transcript --params "$P" --count 1 > /dev/null 2> "$T/narrow.err"
grep -q 'may not read wheel.agent.transcript; its reads: wheel.agent.status' "$T/narrow.err" \
  && pass "a transcript read outside the pane's reads is refused" \
  || fail "a transcript read outside the reads is refused: $(cat "$T/narrow.err")"
w plugin action "$NARROW" wheel.agent.send --args "{\"agent\":\"$AGENT_PANE\",\"text\":\"x\"}" > /dev/null 2> "$T/narrow2.err"
grep -q 'may not invoke wheel.agent.send; its actions: none' "$T/narrow2.err" \
  && pass "a send outside the pane's actions is refused" \
  || fail "a send outside the actions is refused: $(cat "$T/narrow2.err")"
w plugin feed "$PANE" wheel.agent.status --count 1 > /dev/null 2> "$T/noparams.err"
grep -q 'wheel.agent.status takes params {agent: <agent pane id or agent name>}; agent is missing' "$T/noparams.err" \
  && pass "an agent feed read with no agent is refused with what it takes" \
  || fail "an agent feed read with no agent is refused: $(cat "$T/noparams.err")"
w plugin feed "$PANE" wheel.agent.status --params '{"agent":"nobody"}' --count 1 > /dev/null 2> "$T/nobody.err"
[ -s "$T/nobody.err" ] && pass "an agent feed of an agent that does not exist is refused ($(head -1 "$T/nobody.err"))" \
  || fail "an agent feed of a missing agent is refused"
w plugin feed "$PANE" wheel.agent.status --params "{\"agent\":\"$NARROW\"}" --count 1 > /dev/null 2> "$T/notagent.err"
grep -q "$NARROW is not an agent pane" "$T/notagent.err" && pass "an agent feed of a pane that runs no agent is refused" \
  || fail "an agent feed of a non-agent pane is refused: $(cat "$T/notagent.err")"
w plugin action "$PANE" wheel.agent.send --args "{\"agent\":\"$AGENT_PANE\"}" > /dev/null 2> "$T/notext.err"
grep -q 'wheel.agent.send takes args {agent, text}; text is missing' "$T/notext.err" \
  && pass "a send with no text is refused with what it takes" \
  || fail "a send with no text is refused: $(cat "$T/notext.err")"
w plugin action "$PANE" wheel.agent.nope > /dev/null 2> "$T/nope.err"
grep -q 'wheeld has no action wheel.agent.nope; it has wheel.agent.send, wheel.agent.answer' "$T/nope.err" \
  && pass "an action wheeld lacks is refused with the ones it has" \
  || fail "an action wheeld lacks is refused: $(cat "$T/nope.err")"
w plugin feed "$PANE" wheel.agent.nope --count 1 > /dev/null 2> "$T/nofeed.err"
grep -q 'wheeld has no feed wheel.agent.nope; it has wheel.spaces, wheel.panes, wheel.agents, wheel.agent.transcript, wheel.agent.status, wheel.agent.requests' "$T/nofeed.err" \
  && pass "a wheel feed wheeld lacks is refused with the ones it has" \
  || fail "a wheel feed wheeld lacks is refused: $(cat "$T/nofeed.err")"

# 4b streamed chunks are merged into one reply, as the agent stores it, so the value equals what a
# new reader is sent.
w plugin feed "$PANE" wheel.agent.transcript --params "$P" --count 1 > "$T/fresh.jsonl" 2>&1
python3 - "$T/transcript.jsonl" "$T/fresh.jsonl" <<'PYEOF' && pass "the folded live transcript equals a new reader's value" || fail "the folded live transcript equals a new reader's value"
import json, sys
def fold(path):
    v = None
    for line in open(path):
        if not line.strip().startswith("{"):
            continue
        row = json.loads(line)
        if "patch" in row:
            for op in row["patch"]:
                _, f, at = op["path"].split("/")
                if op["op"] == "add": v[f].append(op["value"])
                elif op["op"] == "replace": v[f][int(at)] = op["value"]
                else: del v[f][int(at)]
        else:
            v = row["value"]
    return v
live, fresh = fold(sys.argv[1]), fold(sys.argv[2])
sys.exit(0 if live == fresh and live["events"] else 1)
PYEOF

# 5 two readers of one transcript share one pump: a second reader's first value has the turns.
w plugin feed "$PANE" wheel.agent.transcript --params "{\"agent\":\"$AGENT_PANE\"}" --count 1 > "$T/second.jsonl" 2>&1
folds_to "$T/second.jsonl" "any(e['kind'] == 'user' and e['text'] == 'hello feeds' for e in v['events'])" \
  && pass "a second reader's first value holds the turns so far" \
  || fail "a second reader's first value holds the turns: $(head -c 400 "$T/second.jsonl")"

echo "$FAILS failed"
[ "$FAILS" -eq 0 ]
