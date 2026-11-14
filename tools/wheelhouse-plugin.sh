#!/bin/bash
# Check the wheelhouse plugin's backend (wheel-pmf2.4.7) against a throwaway wheeld: the package
# plugins/wheelhouse is installed through a symlink, a plugin pane of kind wheelhouse/beads reads
# the `wheelhouse.beads` feed of temp projects (made with `bd init`) and invokes
# `wheelhouse.press` and `wheelhouse.run_list`, as the Luau pane does. Prints PASS or FAIL per
# case. Uses no GUI, focus, mouse or keyboard. Everything it starts is killed by pid at the end.
#
#   tools/wheelhouse-plugin.sh         # uses target/debug/{wheeld,wheel,wheelhouse}
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=/private/tmp/claude/wheel/wh
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
  pkill -f "$ROOT/target/debug/wheelhouse backend" 2>/dev/null
}
trap cleanup EXIT

# The projects are made with the real HOME (bd and git read their config there); wheeld, and so
# the backend and the buttons it runs, get a HOME of their own, so ~/.wheel/bead-panes.json is
# the throwaway's.
rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/home/.wheel" "$T/plugins"
# wheeld asks the login shell for PATH, so the shell's startup files are the real ones'"'"'.
for f in .zshenv .zprofile .zshrc .zlogin .bash_profile .bashrc .profile; do
  [ -e "$HOME/$f" ] && ln -s "$HOME/$f" "$T/home/$f"
done
: > "$T/machine.toml"
ln -s "$ROOT/plugins/wheelhouse" "$T/plugins/wheelhouse"
RECORDS="$T/home/.wheel/bead-panes.json"

# A project with beads: an epic with two children, a leaf blocked by one child, a closed one, and
# a leaf to press. Sets ID_<name> for each.
make_project() {
  local dir=$1 prefix=$2
  mkdir -p "$dir"
  git -C "$dir" init -q
  ( cd "$dir" && bd init --quiet --prefix "$prefix" > /dev/null 2>&1 )
}
new_bead() { # dir title [bd create flags...] -> prints the id
  local dir=$1 title=$2
  shift 2
  ( cd "$dir" && bd create "$title" --json "$@" 2>/dev/null ) | python3 -c 'import json, sys; print(json.load(sys.stdin)["id"])'
}

A="$T/proj-a"
B="$T/proj-b"
make_project "$A" a
make_project "$B" b
EPIC=$(new_bead "$A" "The epic" -t epic -p 1)
C1=$(new_bead "$A" "First child" -t task -p 2 --parent "$EPIC")
C2=$(new_bead "$A" "Second child" -t task -p 2 --parent "$EPIC")
BLOCKED=$(new_bead "$A" "Blocked leaf" -t task -p 3)
( cd "$A" && bd dep add "$BLOCKED" "$C1" > /dev/null 2>&1 )
DONE=$(new_bead "$A" "Finished leaf" -t task -p 3)
( cd "$A" && bd close "$DONE" > /dev/null 2>&1 )
LEAF=$(new_bead "$A" "Leaf to press" -t task -p 2)
BB=$(new_bead "$B" "Only bead of b" -t task -p 2)
echo "beads: epic $EPIC children $C1 $C2 blocked $BLOCKED closed $DONE leaf $LEAF; project b: $BB"

cat > "$A/wheel.toml" <<TOML
[beads.task]
start.label = "Implement"
start.run = 'echo started "\$BEAD_ID" "\$BEAD_REPO" > "$T/marker-start"; echo pane-gone'
resume.label = "Continue"
resume.run = 'echo resumed "\$BEAD_ID" > "$T/marker-resume"; echo pane-gone'
focus.label = "Focus"

[beads.epic]
start.label = "Plan"
start.run = 'echo planned > "$T/marker-plan"; echo pane-gone'

[beads.list.mark]
label = "Mark list"
run = 'echo "\$BEAD_IDS" > "$T/marker-list"'
TOML

HOME="$T/home" "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
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
backend_pid() {
  w plugin ls --json --space "$1" 2>/dev/null | python3 -c '
import json, sys
for p in json.load(sys.stdin):
    if p["id"] == "wheelhouse" and p.get("backend", {}).get("pid"):
        print(p["backend"]["pid"])
'
}
backend_pid_not() {
  local now
  now=$(backend_pid "$1")
  [ -n "$now" ] && [ "$now" != "$2" ] && kill -0 "$now" 2>/dev/null
}
# Succeeds when file $1 has at least $2 lines.
lines_at_least() { [ "$(wc -l < "$1" 2>/dev/null || echo 0)" -ge "$2" ]; }
has_file() { [ -s "$1" ]; }

SPACE=$(w space new "$A")
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
w pane kind "$PANE" plugin --plugin wheelhouse --plugin-pane beads > /dev/null
echo "space $SPACE pane $PANE (the package folder is a symlink to $ROOT/plugins/wheelhouse)"
w plugin ls --space "$SPACE" > "$T/ls.txt" 2>&1
grep -q wheelhouse "$T/ls.txt" && pass "wheeld finds the package through a symlinked folder" \
  || fail "wheeld finds the package through a symlinked folder: $(cat "$T/ls.txt")"

feed() { # file dir -> runs `wheel plugin feed` in the background, prints its pid
  w plugin feed "$PANE" wheelhouse.beads --params "{\"dir\":\"$2\"}" --count 200 > "$1" 2> "$1.err" &
  echo $!
}
# Succeeds when file $1 has a row with a patch.
has_patch() { grep -q '"patch"' "$1"; }
rows() { python3 -c '
import json, sys
print(sum(1 for l in open(sys.argv[1]) if l.strip().startswith("{")))' "$1"; }

# 1 the first value has the shape the pane codes against.
FA="$T/feed-a.jsonl"
FEED_A=$(feed "$FA" "$A")
PIDS+=("$FEED_A")
poll 30 grep -q '"beads":\[{' "$FA"
python3 - "$FA" "$A" "$EPIC" "$C1" "$C2" "$BLOCKED" "$DONE" "$LEAF" <<'PY' > "$T/shape.verdict"
import json, sys
path, dirn, epic, c1, c2, blocked, done, leaf = sys.argv[1:]
rows = [json.loads(l) for l in open(path) if l.strip().startswith("{")]
vals = [r for r in rows if r.get("value") and r["value"].get("beads")]
if not vals:
    print("no value with beads: %r" % rows); sys.exit()
r = vals[0]
v = r["value"]
if r.get("params") != {"dir": dirn}:
    print("params not echoed: %r" % r.get("params")); sys.exit()
want = {"now", "name", "path", "has_repo", "error", "problems", "buttons", "scripts", "records", "pressing", "beads"}
if set(v) != want:
    print("keys %r" % sorted(v)); sys.exit()
bead_keys = {"id", "title", "status", "kind", "priority", "parent", "labels", "owner", "updated",
             "blocked_by", "description", "ready", "epic"}
by = {b["id"]: b for b in v["beads"]}
problems = []
if v["name"] != "proj-a" or v["path"] != dirn or v["has_repo"] is not True or v["error"] is not None:
    problems.append("header %r" % {k: v[k] for k in ("name", "path", "has_repo", "error")})
if any(set(b) != bead_keys for b in v["beads"]):
    problems.append("bead keys")
if set(by) != {epic, c1, c2, blocked, done, leaf}:
    problems.append("ids %r" % sorted(by))
elif not (by[epic]["epic"] and by[c1]["parent"] == epic and by[c2]["parent"] == epic
          and by[blocked]["blocked_by"] == [c1] and by[done]["status"] == "closed"
          and by[leaf]["ready"] and not by[blocked]["ready"] and by[c1]["priority"] == 2):
    problems.append("bead fields %r" % by)
t = v["buttons"]["task"]
if not (t and t["start"] == {"label": "Implement", "run": t["start"]["run"]} and t["start"]["run"]
        and t["focus"] == {"label": "Focus", "run": None} and v["buttons"]["epic"]["start"]["label"] == "Plan"
        and v["buttons"]["closed"] is None):
    problems.append("buttons %r" % v["buttons"])
if v["scripts"] != [{"name": "mark", "label": "Mark list"}]:
    problems.append("scripts %r" % v["scripts"])
if v["records"] != {} or v["pressing"] != [] or v["problems"] != []:
    problems.append("records/pressing/problems")
print("ok" if not problems else "; ".join(problems))
PY
check "the feed's first value has the documented shape and echoes params" "$(cat "$T/shape.verdict")"
python3 - "$FA" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip().startswith("{")]
v = next(r["value"] for r in rows if r.get("value") and r["value"].get("beads"))
v = dict(v)
v["beads"] = v["beads"][:2]
print(json.dumps(v)[:900])
PY

# 2 a bd update arrives as a patch.
BEFORE=$(rows "$FA")
( cd "$A" && bd update "$C1" --status in_progress > /dev/null 2>&1 )
poll 10 has_patch "$FA"
python3 - "$FA" "$C1" <<'PY' > "$T/patch.verdict"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip().startswith("{")]
patches = [r for r in rows if "patch" in r and "value" not in r]
if not patches:
    print("no patch row"); sys.exit()
text = json.dumps(patches[0]["patch"])
print("ok" if "in_progress" in text and all(set(op) >= {"op", "path"} for op in patches[0]["patch"]) else text[:300])
PY
check "bd update --status in_progress arrives as a patch" "$(cat "$T/patch.verdict")"

# 3 nothing is sent when nothing changed.
N=$(rows "$FA")
touch "$A/.beads/last-touched"
sleep 2
[ "$(rows "$FA")" = "$N" ] && pass "touching .beads/last-touched with no change sends nothing" \
  || fail "touching .beads/last-touched with no change sends nothing ($N rows, now $(rows "$FA"))"

# 4 a second project gets its own value.
FB="$T/feed-b.jsonl"
FEED_B=$(feed "$FB" "$B")
PIDS+=("$FEED_B")
poll 30 grep -q "\"$BB\"" "$FB"
python3 - "$FB" "$BB" "$B" <<'PY' > "$T/b.verdict"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip().startswith("{")]
v = next((r["value"] for r in rows if r.get("value") and r["value"].get("beads")), None)
ok = v and v["name"] == "proj-b" and [b["id"] for b in v["beads"]] == [sys.argv[2]] and v["buttons"]["task"] is None
print("ok" if ok else "value=%r" % v)
PY
check "a second project directory gets its own value" "$(cat "$T/b.verdict")"
( cd "$B" && bd update "$BB" --title "Renamed in b" > /dev/null 2>&1 )
poll 10 has_patch "$FB"
grep -q "Renamed in b" "$FB" && pass "a change in project b is a patch on b's feed" || fail "a change in project b is a patch on b's feed"
grep -q "Renamed in b" "$FA" && fail "a change in project b does not reach a's feed" || pass "a change in project b does not reach a's feed"

# 5 press on a fresh bead runs the start command, answers with `said`, and records the pane.
w plugin action "$PANE" wheelhouse.press --args "{\"dir\":\"$A\",\"bead\":\"$LEAF\"}" > "$T/press.json" 2> "$T/press.err"
poll 5 has_file "$T/marker-start"
if [ "$(cat "$T/marker-start" 2>/dev/null)" = "started $LEAF proj-a" ] \
  && python3 -c 'import json, sys; r = json.loads(open(sys.argv[1]).readline()); sys.exit(0 if r == {"said": "Implement started for " + sys.argv[2] + ". See the new pane."} else 1)' "$T/press.json" "$LEAF"; then
  pass "press on a fresh bead runs [beads.task] start with BEAD_ID and BEAD_REPO and answers said"
else
  fail "press on a fresh bead: $(cat "$T/press.json") $(cat "$T/press.err") marker=$(cat "$T/marker-start" 2>/dev/null)"
fi
cat "$T/press.json"
poll 10 grep -q "\"pane-gone\"" "$FA"
grep -q 'pane-gone' "$FA" && pass "the press's record reaches the feed (records changed on disk)" \
  || fail "the press's record reaches the feed"
grep -q "\"pressing\":\[\"$LEAF\"\]" "$FA" && echo "(the claim was visible in pressing)" || echo "(the claim was too short to show in pressing)"
python3 - "$FA" "$LEAF" <<'PY' > "$T/pressing.verdict"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip().startswith("{")]
cur = None
for r in rows:
    if r.get("value") is not None:
        cur = r["value"]
    elif r.get("patch") and cur is not None:
        import copy
        for op in r["patch"]:
            parts = [p.replace("~1", "/").replace("~0", "~") for p in op["path"].lstrip("/").split("/")]
            node = cur
            for p in parts[:-1]:
                node = node[int(p)] if isinstance(node, list) else node[p]
            last = parts[-1]
            if op["op"] == "remove":
                if isinstance(node, list): node.pop(int(last))
                else: node.pop(last)
            else:
                val = op["value"]
                if isinstance(node, list):
                    if last == "-": node.append(val)
                    elif op["op"] == "add": node.insert(int(last), val)
                    else: node[int(last)] = val
                else: node[last] = val
ok = cur and cur["pressing"] == [] and cur["records"].get(sys.argv[2], {}).get("pane") == "pane-gone"
print("ok" if ok else "final %r" % (cur and (cur["pressing"], cur["records"])))
PY
check "patches applied to the first value give the current value (no claim left, record kept)" "$(cat "$T/pressing.verdict")"

# 6 the recorded pane is gone, so the next press resumes.
w plugin action "$PANE" wheelhouse.press --args "{\"dir\":\"$A\",\"bead\":\"$LEAF\"}" > "$T/resume.json" 2> "$T/resume.err"
poll 5 has_file "$T/marker-resume"
grep -q "resumed $LEAF" "$T/marker-resume" 2>/dev/null && grep -q 'Continue started' "$T/resume.json" \
  && pass "press on a bead with a record whose pane is gone runs resume" \
  || fail "press resume: $(cat "$T/resume.json") $(cat "$T/resume.err")"

# 7 a recorded pane that is open is focused, through the wheel CLI the backend reaches.
python3 - "$RECORDS" "$A" "$C2" "$PANE" <<'PY'
import json, sys
path, proj, bead, pane = sys.argv[1:]
try:
    data = json.load(open(path))
except Exception:
    data = {}
data["%s\n%s" % (proj, bead)] = {"pane": pane, "conversation": ""}
json.dump(data, open(path + ".tmp", "w"))
import os
os.rename(path + ".tmp", path)
PY
poll 10 grep -q "/records/$C2" "$FA"
grep -q "/records/$C2" "$FA" && pass "a record written to bead-panes.json reaches the feed" \
  || fail "a record written to bead-panes.json reaches the feed"
w plugin action "$PANE" wheelhouse.press --args "{\"dir\":\"$A\",\"bead\":\"$C2\"}" > "$T/focus.json" 2> "$T/focus.err"
python3 - "$T/focus.json" "$PANE" "$C2" <<'PY' > "$T/focus.verdict"
import json, sys
r = json.loads(open(sys.argv[1]).readline())
print("ok" if r == {"said": "Focused %s for %s." % (sys.argv[2], sys.argv[3])} else "result=%r" % r)
PY
check "press on a bead whose recorded pane is open focuses it (wheel pane focus)" "$(cat "$T/focus.verdict")"
cat "$T/focus.json"

# 8 an epic uses [beads.epic].
w plugin action "$PANE" wheelhouse.press --args "{\"dir\":\"$A\",\"bead\":\"$EPIC\"}" > "$T/epic.json" 2> "$T/epic.err"
poll 5 has_file "$T/marker-plan"
[ "$(cat "$T/marker-plan" 2>/dev/null)" = planned ] && grep -q 'Plan started' "$T/epic.json" \
  && pass "press on an epic runs [beads.epic] start" || fail "press on an epic: $(cat "$T/epic.json") $(cat "$T/epic.err")"

# 9 run_list runs a [beads.list.<name>] script with BEAD_IDS.
w plugin action "$PANE" wheelhouse.run_list --args "{\"dir\":\"$A\",\"script\":\"mark\",\"ids\":[\"$C1\",\"$C2\"]}" > "$T/list.json" 2> "$T/list.err"
poll 5 has_file "$T/marker-list"
if [ "$(cat "$T/marker-list" 2>/dev/null)" = "$C1 $C2" ] && grep -q 'Mark list started over 2 beads' "$T/list.json"; then
  pass "run_list runs the list script with BEAD_IDS and answers said"
else
  fail "run_list: $(cat "$T/list.json") $(cat "$T/list.err") marker=$(cat "$T/marker-list" 2>/dev/null)"
fi
cat "$T/list.json"

# 10 errors.
w plugin action "$PANE" wheelhouse.press --args "{\"dir\":\"$A\",\"bead\":\"nope-1\"}" > /dev/null 2> "$T/nobead.err"
grep -q 'press failed: no bead nope-1' "$T/nobead.err" && pass "press on an unknown bead is an error" \
  || fail "press on an unknown bead: $(cat "$T/nobead.err")"
cat "$T/nobead.err"
w plugin action "$PANE" wheelhouse.run_list --args "{\"dir\":\"$A\",\"script\":\"nope\",\"ids\":[\"$C1\"]}" > /dev/null 2> "$T/noscript.err"
grep -q 'no list button nope' "$T/noscript.err" && pass "run_list with an unknown script is an error" \
  || fail "run_list with an unknown script: $(cat "$T/noscript.err")"
w plugin action "$PANE" wheelhouse.press --args "{\"bead\":\"$C1\"}" > /dev/null 2> "$T/nodir.err"
grep -q 'needs args.dir' "$T/nodir.err" && pass "an action with no dir is an error" || fail "an action with no dir: $(cat "$T/nodir.err")"
w plugin action "$PANE" wheelhouse.shutdown_now > /dev/null 2> "$T/noact.err"
grep -q 'may not invoke wheelhouse.shutdown_now; its actions: wheelhouse.press, wheelhouse.run_list' "$T/noact.err" && pass "an action the manifest does not list is refused" || fail "unknown action: $(cat "$T/noact.err")"

# 11 a change to wheel.toml reaches the feed.
sed -i '' 's/label = "Mark list"/label = "Mark all"/' "$A/wheel.toml"
poll 10 grep -q 'Mark all' "$FA"
grep -q 'Mark all' "$FA" && pass "an edit of wheel.toml reaches the feed" || fail "an edit of wheel.toml reaches the feed"

# 12 kill -9 of the backend: stale, then a fresh value.
OLD=$(backend_pid "$SPACE")
kill -9 "$OLD" 2>/dev/null
poll 20 backend_pid_not "$SPACE" "$OLD"
NEW=$(backend_pid "$SPACE")
[ -n "$NEW" ] && [ "$NEW" != "$OLD" ] && pass "wheeld restarts the backend after kill -9 ($OLD to $NEW)" || fail "backend restart ($OLD, now '$NEW')"
sleep 1.5
python3 - "$FA" <<'PY' > "$T/stale.verdict"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip().startswith("{")]
i = next((i for i, r in enumerate(rows) if r.get("stale")), None)
if i is None:
    print("no stale row")
else:
    later = [r for r in rows[i + 1:] if r.get("value") and r["value"].get("beads") and not r.get("stale")]
    print("ok" if later else "no fresh value after the stale one")
PY
check "the reader gets a stale value, then a fresh one from the new process" "$(cat "$T/stale.verdict")"
( cd "$A" && bd update "$C2" --title "After the restart" > /dev/null 2>&1 )
poll 10 grep -q 'After the restart' "$FA"
grep -q 'After the restart' "$FA" && pass "changes still arrive after the restart (watching resumed)" || fail "changes still arrive after the restart"

w plugin logs wheelhouse --space "$SPACE" --tail 5 > "$T/log.txt" 2>&1
echo "--- plugin log tail"
cat "$T/log.txt"

echo
if [ "$FAILS" -eq 0 ]; then echo "all checks passed"; else echo "$FAILS check(s) failed"; fi
exit "$FAILS"
