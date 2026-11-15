#!/bin/bash
# Check Wheel's command backend (wheel-pmf2.5.3) against a throwaway wheeld: the fixture
# tools/fixtures/plugins/commands has a manifest and a pane script and no backend; its [commands]
# table gives it `wheel-runner commands`. Reads its feeds and runs its actions as its pane, through
# `wheel plugin feed` and `wheel plugin action`. Prints PASS or FAIL per check. Uses no GUI, focus,
# mouse or keyboard. Everything it starts is killed by pid at the end.
#
#   tools/plugin-commands.sh            # uses target/debug/wheeld, wheel and wheel-runner
#   WHEELD=... WHEEL=... tools/plugin-commands.sh
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=/private/tmp/claude/wheel/pcmd
PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
PKG=$T/proj/.wheel/plugins/cmd
FAILS=0
PIDS=()

pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }
check() { if [ "$2" = ok ]; then pass "$1"; else fail "$1 ($2)"; fi; }

cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null; done
  sleep 0.5
  for pid in "${PIDS[@]}"; do kill -9 "$pid" 2>/dev/null; done
}
trap cleanup EXIT

rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/proj/.wheel/plugins" "$T/other"
: > "$T/machine.toml"
cp -R "$ROOT/tools/fixtures/plugins/commands" "$PKG"

"$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
  --log-dir "$T/logs" --cli "$WHEEL" > "$T/wheeld.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }

SPACE=$(w space new "$T/proj")
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
w pane kind "$PANE" plugin --plugin cmd --plugin-pane main > /dev/null
echo "space $SPACE pane $PANE"

# 1 the [commands] table gives the package the command backend.
w plugin ls --json --space "$SPACE" > "$T/ls.json"
check "plugin ls names wheel-runner commands as the backend" "$(python3 -c '
import json, sys
p = next(p for p in json.load(open(sys.argv[1])) if p["id"] == "cmd")
b = p.get("backend") or {}
print("ok" if b.get("command") == "wheel-runner commands" and b.get("watch") == ["wheel-plugin.toml"] else p)
' "$T/ls.json")"

# 2 case 1: a JSON feed on an interval; the value changes and arrives as a patch.
w plugin feed "$PANE" cmd.now --count 4 > "$T/now.jsonl" 2> "$T/now.err" &
NOWPID=$!
PIDS+=($NOWPID)
for _ in $(seq 1 80); do [ "$(wc -l < "$T/now.jsonl")" -ge 4 ] && break; sleep 0.1; done
check "cmd.now: whole value, then a patch per second" "$(python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
first = next((r for r in rows if r.get("value") and r["value"].get("value")), None)
patches = [r for r in rows if r.get("patch")]
ok = first and isinstance(first["value"]["value"]["now"], int) and first["value"]["error"] is None and patches and all(op["path"] == "/value/now" for p in patches for op in p["patch"])
print("ok" if ok else rows)
' "$T/now.jsonl")"

# 3 a lines feed with a watch: no note yet is an empty list.
w plugin feed "$PANE" cmd.note > "$T/note.jsonl" 2> "$T/note.err" &
PIDS+=($!)
# Rebuild the note feed's value from its lines and test it with the Python expression $1 (v is the value).
note_is() {
  python3 - "$T/note.jsonl" "$1" <<'PYEOF'
import copy, json, sys
v = None
for line in open(sys.argv[1]):
    if not line.strip():
        continue
    r = json.loads(line)
    if "patch" in r:
        for op in r["patch"]:
            parts = [p for p in op["path"].split("/")[1:]]
            at = v
            for p in parts[:-1]:
                at = at[int(p)] if isinstance(at, list) else at[p]
            last = parts[-1]
            if op["op"] in ("add", "replace"):
                if isinstance(at, list):
                    if last == "-":
                        at.append(op["value"])
                    elif op["op"] == "add":
                        at.insert(int(last), op["value"])
                    else:
                        at[int(last)] = op["value"]
                else:
                    at[last] = op["value"]
            elif op["op"] == "remove":
                del at[int(last) if isinstance(at, list) else last]
    elif "value" in r:
        v = copy.deepcopy(r["value"])
sys.exit(0 if v is not None and eval(sys.argv[2]) else 1)
PYEOF
}
poll() {
  local n=$(( $1 * 10 ))
  shift
  for _ in $(seq 1 "$n"); do "$@" > /dev/null 2>&1 && return 0; sleep 0.1; done
  return 1
}
poll 5 note_is 'v == {"value": [], "error": None}' && pass "cmd.note starts as an empty list" || fail "cmd.note starts as an empty list ($(cat "$T/note.jsonl"))"

# 4 case 2: an action runs with its args in the environment (quotes intact), answers its output,
# and the feeds run again.
w plugin action "$PANE" cmd.write --args '{"text": "it'"'"'s \"one\""}' > "$T/write.json" 2>&1
check "cmd.write answers code, stdout and stderr" "$(python3 -c '
import json, sys
r = json.load(open(sys.argv[1]))
print("ok" if r == {"code": 0, "stdout": "wrote it'"'"'s \"one\"\n", "stderr": ""} else r)
' "$T/write.json")"
poll 3 note_is "v[\"value\"] == ['it\\'s \"one\"']" && pass "cmd.note shows the written line after the action" || fail "cmd.note after write ($(tail -3 "$T/note.jsonl"))"

# 5 an action with no watch: the feed runs again after it. Count the note feed's updates before
# and after an action that changes nothing the note reads.
before=$(wc -l < "$T/note.jsonl")
w plugin action "$PANE" cmd.mend > /dev/null 2>&1
sleep 1
after=$(wc -l < "$T/note.jsonl")
[ "$after" = "$before" ] && pass "an action that changes nothing sends no update" || fail "unchanged value sent an update ($before -> $after)"

# 6 a watched file changed by hand reruns the feed.
printf 'by hand\n' >> "$PKG/note.txt"
poll 3 note_is "v[\"value\"][-1] == 'by hand'" && pass "a change to a watched file reruns the feed" || fail "watch ($(tail -3 "$T/note.jsonl"))"

# 7 case 3: a failing run is the feed's error (its stderr), the last good value is kept, and the
# next good run clears the error.
w plugin action "$PANE" cmd.break --args '{"why": "on purpose"}' > /dev/null 2>&1
poll 3 note_is "v[\"error\"] == 'exit 4: note is broken: on purpose' and v[\"value\"][-1] == 'by hand'" \
  && pass "a non-zero exit is the feed's error, with the last good value kept" || fail "error ($(tail -3 "$T/note.jsonl"))"
w plugin action "$PANE" cmd.mend > /dev/null 2>&1
poll 3 note_is "v[\"error\"] is None" && pass "the next good run clears the error" || fail "clear ($(tail -3 "$T/note.jsonl"))"

# 8 a failing action answers its exit code and both streams.
w plugin action "$PANE" cmd.refuse --args '{"why": "no"}' > "$T/refuse.json" 2>&1
check "a failing action answers its code, stdout and stderr" "$(python3 -c '
import json, sys
r = json.load(open(sys.argv[1]))
print("ok" if r == {"code": 7, "stdout": "", "stderr": "refused: no\n"} else r)
' "$T/refuse.json")"

# 9 the backend's log has the failures.
w plugin logs cmd --space "$SPACE" > "$T/log.txt" 2>&1
grep -q 'feed note {}: exit 4: note is broken' "$T/log.txt" && grep -q 'action refuse' "$T/log.txt" \
  && pass "the plugin log names the failed feed run and action" || fail "log ($(tail -5 "$T/log.txt"))"

# 10 a manifest edit restarts the backend with the new table (watch = wheel-plugin.toml).
pid1=$(w plugin ls --json --space "$SPACE" | python3 -c 'import json,sys; print(next(p for p in json.load(sys.stdin) if p["id"]=="cmd")["backend"].get("pid",""))')
sed -i '' 's|^run = .rm -f fail.$|run = '"'"'rm -f fail; echo mended'"'"'|' "$PKG/wheel-plugin.toml"
mended() { w plugin action "$PANE" cmd.mend 2>/dev/null | grep -q '"stdout":"mended\\n"'; }
poll 8 mended && pass "a manifest edit restarts the backend with the edited command" || fail "manifest edit (pid was $pid1)"

# 11 a bad [commands] table is refused by wheeld with the reason.
printf '\n[commands.feeds.bad]\nrun = "x"\nevry = 1\n' >> "$PKG/wheel-plugin.toml"
poll 3 bash -c "\"$WHEEL\" --url ws://127.0.0.1:$PORT/ws --token-file $T/support/token plugin ls --space $SPACE 2>&1 | grep -q evry" \
  && pass "a bad [commands] table shows its error in plugin ls" || fail "bad table ($(w plugin ls --space "$SPACE" 2>&1 | head -5))"

echo
[ $FAILS -eq 0 ] && echo "ALL PASS" || echo "$FAILS FAILED"
exit $FAILS
