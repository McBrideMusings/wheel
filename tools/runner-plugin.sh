#!/bin/bash
# Check the runner plugin (wheel-pmf2.5.1.1) against a throwaway wheeld: the built-in package `runner`
# (plugins/runner, compiled into wheeld) starts its backend `wheel-runner backend` from the bin
# directory wheeld gives a backend, a plugin pane of kind runner/runner reads `runner.commands` and
# `runner.log` and invokes `runner.run` and `runner.stop` as the Luau pane does, through
# `wheel plugin feed` and `wheel plugin action`. Cases: the project's [runner] commands (groups from
# nested tables, the machine's too) arrive; Run starts a command, its run shows running and its log
# grows through patches; a confirm command starts nothing until its answer; a failing guard answers
# with its message; Stop ends a run and its processes; a status check's reading arrives; a project
# other than the pane's is its own value. Prints PASS or FAIL per check. Uses no GUI, focus, mouse
# or keyboard. Everything it starts is killed by pid at the end.
#
#   tools/runner-plugin.sh      # needs cargo build -p wheeld -p wheel-cli -p wheel-runner --bins
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
RUNNER=${RUNNER:-$ROOT/target/debug/wheel-runner}
T=/private/tmp/claude/wheel/rp
PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
FAILS=0
PIDS=()

pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }
check() { if [ "$2" = ok ]; then pass "$1"; else fail "$1 ($2)"; fi; }

cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null; done
  sleep 0.5
  for pid in "${PIDS[@]}"; do kill -9 "$pid" 2>/dev/null; done
  pkill -f "$T/home" 2>/dev/null
}
trap cleanup EXIT

for f in "$WHEELD" "$WHEEL" "$RUNNER"; do
  [ -x "$f" ] || { echo "missing $f: run cargo build -p wheeld -p wheel-cli -p wheel-runner --bins" >&2; exit 2; }
done
[ "$(dirname "$WHEEL")" = "$(dirname "$RUNNER")" ] || { echo "wheel and wheel-runner must sit in one directory: wheeld links the runner beside the CLI" >&2; exit 2; }

rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/home/.wheel" "$T/a" "$T/b"
for f in .zshenv .zprofile .zshrc .zlogin .bash_profile .bashrc .profile; do
  [ -e "$HOME/$f" ] && ln -s "$HOME/$f" "$T/home/$f"
done
# The machine's [runner] table is in the file wheeld and the backend both read: HOME is the throwaway's.
cat > "$T/home/.wheel/wheel.toml" <<TOML
[runner.machine]
hello = "echo hello-from-machine"
TOML
cat > "$T/a/wheel.toml" <<TOML
[runner]
count = { command = "for i in 1 2 3 4; do echo line\$i; sleep 0.5; done" }
long = "sleep 3001 & wait"
fail = "echo failing; exit 7"
danger = { command = "echo danger-ran > $T/danger-marker", confirm = "Really run it?" }
guarded = { command = "echo guarded-ran", requires = ["never"] }
quick = "echo quick"
up = { command = "echo up-ran", status = { kind = "command", run = "echo v42" } }

[runner.prototypes.terrarium]
dev = "echo dev-ran"


[guards.never]
run = "false"
fail = "never passes"
TOML
cat > "$T/b/wheel.toml" <<TOML
[runner]
only-b = "echo b"
TOML

HOME="$T/home" "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/home/.wheel/wheel.toml" \
  --log-dir "$T/logs" --cli "$WHEEL" > "$T/wheeld.out" 2>&1 &
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

SPACE=$(w space new "$T/a")
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
w pane kind "$PANE" plugin --plugin runner --plugin-pane runner > /dev/null
echo "space $SPACE pane $PANE"

# Folds the feed lines in file $1 the way a pane does (a `value` replaces; a `patch` applies add, replace and remove) and
# prints the final value as JSON.
fold() {
  python3 - "$1" <<'PYEOF'
import json, sys
v = None
def walk(doc, parts):
    for p in parts:
        doc = doc[int(p)] if isinstance(doc, list) else doc[p]
    return doc
for line in open(sys.argv[1]):
    line = line.strip()
    if not line.startswith("{"):
        continue
    row = json.loads(line)
    if "patch" in row:
        for op in row["patch"]:
            parts = op["path"].lstrip("/").split("/")
            parent, last = walk(v, parts[:-1]), parts[-1]
            if op["op"] == "add":
                if isinstance(parent, list):
                    parent.append(op["value"]) if last == "-" else parent.insert(int(last), op["value"])
                else:
                    parent[last] = op["value"]
            elif op["op"] == "replace":
                parent[int(last) if isinstance(parent, list) else last] = op["value"]
            elif op["op"] == "remove":
                del parent[int(last) if isinstance(parent, list) else last]
            else:
                raise SystemExit(f"unexpected op {op}")
    else:
        v = row.get("value")
print(json.dumps(v))
PYEOF
}
# Succeeds when the folded value of file $1 satisfies the Python expression $2 over `v`.
holds() { fold "$1" | python3 -c 'import json, sys; v = json.load(sys.stdin); sys.exit(0 if v is not None and eval(sys.argv[1]) else 1)' "$2"; }
has_patch() { grep -q '"patch"' "$1"; }
act() { w plugin action "$PANE" "$1" --args "$2"; }
field() { python3 -c 'import json, sys; print(json.loads(sys.stdin.read())[sys.argv[1]])' "$1"; }

# 1 the Runner pane lists the space's commands: the project's, in groups the nested tables give, and the machine's.
FC="$T/commands.jsonl"
w plugin feed "$PANE" runner.commands --params "{\"dir\":\"$T/a\"}" --count 400 > "$FC" 2> "$FC.err" &
PIDS+=($!)
poll 30 grep -q '"runners"' "$FC"
holds "$FC" 'v["dir"] == "'"$T"'/a" and [r["name"] for r in v["runners"]] == ["machine.hello", "count", "long", "fail", "danger", "guarded", "quick", "up", "prototypes.terrarium.dev"]' \
  && pass "the feed lists the machine's commands and the project's, in file order" || fail "the feed lists the machine's commands and the project's: $(fold "$FC" | head -c 600)"
holds "$FC" 'next(r for r in v["runners"] if r["name"] == "prototypes.terrarium.dev")["group"] == ["prototypes", "terrarium"] and next(r for r in v["runners"] if r["name"] == "machine.hello")["origin"] == "machine" and next(r for r in v["runners"] if r["name"] == "danger")["confirm"] == "Really run it?"' \
  && pass "nested tables give a command its groups; origin and confirm are in the row" || fail "groups, origin and confirm: $(fold "$FC" | head -c 600)"
w plugin ls --space "$SPACE" > "$T/ls.txt" 2>&1
grep -q 'runner' "$T/ls.txt" && grep -q 'running' "$T/ls.txt" && pass "plugin ls shows the built-in runner with its backend running" || fail "plugin ls: $(cat "$T/ls.txt")"
[ -L "$T/support/shell/bin/wheel-runner" ] || [ -L "$(find "$T/support" -name wheel-runner -maxdepth 4 | head -1)" ] \
  && pass "wheeld linked wheel-runner into the bin directory beside wheel" || fail "wheel-runner is not linked in the bin directory"

# 2 Run starts a command; its run shows running and its log grows live.
RES=$(act runner.run "{\"dir\":\"$T/a\",\"name\":\"count\"}")
ID=$(echo "$RES" | field id)
[ "$(echo "$RES" | field started)" = True ] && pass "runner.run answers started with a run id ($ID)" || fail "runner.run: $RES"
poll 10 holds "$FC" 'any(r["name"] == "count" and r["state"] == "running" for r in v["runs"]) and v["now"] > 0'
holds "$FC" 'any(r["id"] == "'"$ID"'" and r["state"] in ("running", "exited") for r in v["runs"])' \
  && pass "the commands feed shows the run (running, with now set)" || fail "run not in feed: $(fold "$FC" | head -c 600)"
FL="$T/log.jsonl"
w plugin feed "$PANE" runner.log --params "{\"dir\":\"$T/a\",\"id\":\"$ID\"}" --count 200 > "$FL" 2> "$FL.err" &
PIDS+=($!)
poll 20 holds "$FL" 'v["state"] == "exited" and len(v["lines"]) == 4'
holds "$FL" 'v["state"] == "exited" and v["exit_code"] == 0 and [l.split("  ", 1)[1] for l in v["lines"]] == ["line1", "line2", "line3", "line4"]' \
  && pass "the log feed ends with the 4 lines and state exited, exit 0" || fail "log feed: $(fold "$FL" | head -c 600)"
has_patch "$FL" && grep -c '"patch"' "$FL" | awk '{exit !($1 >= 2)}' \
  && pass "the log grew through patches ($(grep -c '"patch"' "$FL") of them), not whole values" || fail "the log arrived as whole values: $(cat "$FL" | head -c 400)"
poll 10 holds "$FC" 'any(r["id"] == "'"$ID"'" and r["state"] == "exited" and r["exit_code"] == 0 for r in v["runs"]) and v["now"] == 0'
holds "$FC" 'any(r["id"] == "'"$ID"'" and r["state"] == "exited" and r["exit_code"] == 0 for r in v["runs"]) and v["now"] == 0' \
  && pass "when the run ends its row says exited 0 and the clock stops (now is 0)" || fail "ended run: $(fold "$FC" | head -c 600)"

# a failing command shows its exit code
RES=$(act runner.run "{\"dir\":\"$T/a\",\"name\":\"fail\"}")
FID=$(echo "$RES" | field id)
poll 15 holds "$FC" 'any(r["id"] == "'"$FID"'" and r["state"] == "exited" and r["exit_code"] == 7 for r in v["runs"])'
holds "$FC" 'any(r["id"] == "'"$FID"'" and r["exit_code"] == 7 for r in v["runs"])' && pass "a failing command's row shows exit 7" || fail "exit 7: $(fold "$FC" | head -c 600)"

# 3 a confirm command asks and starts nothing until it is answered with its command text.
RES=$(act runner.run "{\"dir\":\"$T/a\",\"name\":\"danger\"}")
python3 -c 'import json, sys; r = json.loads(sys.argv[1]); sys.exit(0 if r == {"started": False, "confirm": "Really run it?", "command": "echo danger-ran > '"$T"'/danger-marker"} else 1)' "$RES" \
  && pass "an unanswered confirm command answers {started:false, confirm, command}" || fail "confirm answer: $RES"
sleep 1
[ ! -e "$T/danger-marker" ] && pass "the confirm command started nothing (Cancel is a pane-only step)" || fail "the confirm command ran without an answer"
RES=$(act runner.run "{\"dir\":\"$T/a\",\"name\":\"danger\",\"confirmed\":\"echo danger-ran > $T/danger-marker\"}")
[ "$(echo "$RES" | field started)" = True ] && poll 10 test -e "$T/danger-marker" && pass "answered with its command text, it runs" || fail "answered confirm: $RES"
RES=$(act runner.run "{\"dir\":\"$T/a\",\"name\":\"danger\",\"confirmed\":\"a different command\"}")
[ "$(echo "$RES" | field started)" = False ] && pass "an answer for another command text asks again" || fail "stale answer: $RES"

# a failing guard answers its message
RES=$(act runner.run "{\"dir\":\"$T/a\",\"name\":\"guarded\"}")
[ "$(echo "$RES" | field guard)" = "never: never passes" ] && pass "a failing guard answers its message" || fail "guard: $RES"
w plugin action "$PANE" runner.run --args "{\"dir\":\"$T/a\",\"name\":\"nope\"}" > "$T/nope.txt" 2>&1
grep -q "no runner named nope" "$T/nope.txt" && pass "an unknown command is an error naming it" || fail "unknown command: $(cat "$T/nope.txt")"

# 4 Stop ends a running run and its processes, and its row shows it stopped.
RES=$(act runner.run "{\"dir\":\"$T/a\",\"name\":\"long\"}")
LID=$(echo "$RES" | field id)
poll 10 holds "$FC" 'any(r["id"] == "'"$LID"'" and r["state"] == "running" for r in v["runs"])'
pgrep -f "[s]leep 3001" > /dev/null && pass "the long run's sleep is running" || fail "no sleep 3001 process"
RES=$(act runner.stop "{\"dir\":\"$T/a\",\"id\":\"$LID\"}")
[ "$(echo "$RES" | field stopping)" = "$LID" ] && pass "runner.stop answers stopping <id>" || fail "stop: $RES"
poll 15 holds "$FC" 'any(r["id"] == "'"$LID"'" and r["state"] == "stopped" for r in v["runs"])'
holds "$FC" 'any(r["id"] == "'"$LID"'" and r["state"] == "stopped" and r["exit_code"] is not None for r in v["runs"])' \
  && pass "the run's row says stopped with an exit code" || fail "stopped row: $(fold "$FC" | head -c 600)"
poll 5 bash -c '! pgrep -f "[s]leep 3001" > /dev/null'
pgrep -f "[s]leep 3001" > /dev/null && fail "sleep 3001 survived the stop" || pass "the stopped run's processes are gone"
w plugin action "$PANE" runner.stop --args "{\"dir\":\"$T/a\",\"id\":\"$LID\"}" > "$T/stop2.txt" 2>&1
grep -q "is not running" "$T/stop2.txt" && pass "stopping a run that is over is an error" || fail "stop twice: $(cat "$T/stop2.txt")"

# 6 a status check arrives with its reading on the commands feed.
poll 5 holds "$FC" '"up" in v["checks"]'
holds "$FC" 'v["checks"].get("up") == {"up": True, "text": "v42"} and next(r for r in v["runners"] if r["name"] == "up")["status"] is True' \
  && pass "a status check reads up with its first line, v42" || fail "status check: $(fold "$FC" | head -c 600)"

# the pane's project and another project are two values
FB="$T/commands-b.jsonl"
w plugin feed "$PANE" runner.commands --params "{\"dir\":\"$T/b\"}" --count 5 > "$FB" 2> "$FB.err" &
PIDS+=($!)
poll 20 grep -q '"runners"' "$FB"
holds "$FB" '[r["name"] for r in v["runners"]] == ["machine.hello", "only-b"] and v["runs"] == []' \
  && pass "another project's feed lists its own commands and none of a's runs" || fail "project b: $(fold "$FB" | head -c 600)"

# a wheel.toml edit reaches the feed without a poll
FB2="$T/commands-b2.jsonl"
w plugin feed "$PANE" runner.commands --params "{\"dir\":\"$T/b\"}" --count 20 > "$FB2" 2> "$FB2.err" &
PIDS+=($!)
poll 20 grep -q '"runners"' "$FB2"
printf '[runner]\nonly-b = "echo b"\nadded-later = "echo late"\n' > "$T/b/wheel.toml"
poll 20 holds "$FB2" '"added-later" in [r["name"] for r in v["runners"]]'
holds "$FB2" '"added-later" in [r["name"] for r in v["runners"]]' && pass "an edit to wheel.toml arrives as a patch" || fail "wheel.toml edit: $(fold "$FB2" | head -c 400)"

w plugin logs runner --space "$SPACE" --tail 5 > "$T/log.txt" 2>&1
echo "--- plugin log tail"
cat "$T/log.txt"

echo
if [ "$FAILS" -eq 0 ]; then echo "all checks passed"; else echo "$FAILS check(s) failed"; fi
exit "$FAILS"
