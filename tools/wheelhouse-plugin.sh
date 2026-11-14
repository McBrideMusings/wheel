#!/bin/bash
# Check the wheelhouse plugin's backend (wheel-pmf2.4.7) against a throwaway wheeld: the package
# plugins/wheelhouse is installed through a symlink, a plugin pane of kind wheelhouse/beads reads
# the `wheelhouse.beads` feed of temp projects (made with `bd init`) and invokes
# `wheelhouse.press` and `wheelhouse.run_list`, as the Luau pane does; a pane of kind
# wheelhouse/repo reads `wheelhouse.repo` of a temp git repo with an origin and invokes
# `wheelhouse.branch_delete` and `wheelhouse.run_list` with view repo; a pane of kind wheelhouse/diff
# reads `wheelhouse.diff` of two temp git repos (an edit and a new file arrive, a write under a
# git-ignored directory does not once a tracked file's change has shown the watch is live, the
# other repo is untouched, and a nested tracked file's edit arrives although a beads feed opened the
# same checkout first); a pane of kind wheelhouse/prs reads `wheelhouse.prs` and `wheelhouse.pr_detail`
# through a fake `gh` and invokes `wheelhouse.pr_merge`, `pr_draft`, `pr_close` and `run_list`. Prints PASS or FAIL per
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

# A fake `gh` for the PRs cases, first on the PATH wheeld and so the backend get: it logs each call to
# $T/gh.log and answers `repo view`, `api user` and the two GraphQL queries with canned PRs; `pr merge|close|ready|checkout`
# succeed. wheeld asks the login shell for PATH, whose startup files are the real ones, so the last one
# it reads (.zlogin) puts the fake first.
mkdir -p "$T/bin"
cat > "$T/bin/gh" <<'PY'
#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
log = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "gh.log")
query = next((a[len("query="):] for a in args if a.startswith("query=")), "")
number = next((a[len("number="):] for a in args if a.startswith("number=")), "")
if query:
    shown = ["api", "graphql", "detail " + number if "pullRequest(number" in query else "list"]
else:
    shown = args
with open(log, "a") as f:
    f.write(" ".join(shown) + "\n")
# Every answer differs from the one before (updatedAt counts the calls so far), so a feed read again always has news; a
# merged or closed PR is gone from the list.
stamp = sum(1 for _ in open(log)) % 60
gone = os.path.join(os.path.dirname(log), "gh-gone")
if args[:1] == ["pr"] and args[1:2] in (["merge"], ["close"]):
    open(gone + "-" + args[2], "w").close()
def node(n, title, author, draft, checks, asks_me):
    return {"number": n, "title": title, "bodyText": "Body of %d" % n, "headRefName": "branch-%d" % n, "isDraft": draft,
            "mergeable": "MERGEABLE", "updatedAt": "2026-11-01T00:%02d:00Z" % stamp, "reviewDecision": None, "url": "https://github.com/o/fixture/pull/%d" % n,
            "additions": 5, "deletions": 2, "changedFiles": 1, "author": {"login": author, "__typename": "User"},
            "assignees": {"nodes": []},
            "reviewRequests": {"nodes": [{"requestedReviewer": {"login": "me"}}] if asks_me else []},
            "labels": {"nodes": [{"name": "bug"}]}, "reviews": {"nodes": []}, "comments": {"nodes": []}, "reviewThreads": {"nodes": []},
            "commits": {"totalCount": 1, "nodes": [{"commit": {"committedDate": "2026-11-01T00:00:00Z", "statusCheckRollup": {"state": checks}}}]}}
if args[:2] == ["repo", "view"]:
    print("o/fixture")
elif args[:2] == ["api", "user"]:
    print("me")
elif args[:2] == ["api", "graphql"] and "pullRequest(number" in query:
    d = node(int(number), "PR %s" % number, "me", False, "SUCCESS", False)
    d.update({"body": "## Plan\n\n- first item\n- second item\n\n![shot](https://example.com/shot.png)\n\nDone.",
              "state": "OPEN", "merged": False, "mergeStateStatus": "CLEAN", "baseRefName": "main", "isCrossRepository": False,
              "createdAt": "2026-10-30T00:00:00Z",
              "latestReviews": {"nodes": []},
              "reviews": {"nodes": []},
              "comments": {"nodes": [{"author": {"login": "zed"}, "createdAt": "2026-10-31T00:00:00Z", "body": "Looks fine"}]},
              "history": {"totalCount": 1, "nodes": [{"commit": {"abbreviatedOid": "abc1234", "messageHeadline": "Do it", "committedDate": "2026-10-31T00:00:00Z"}}]},
              "files": {"totalCount": 1, "nodes": [{"path": "src/a.rs", "additions": 5, "deletions": 2}]},
              "checks": {"nodes": [{"commit": {"statusCheckRollup": {"state": "SUCCESS", "contexts": {"nodes": [{"__typename": "CheckRun", "name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"}]}}}}]}})
    print(json.dumps({"data": {"repository": {"pullRequest": d}}}))
elif args[:2] == ["api", "graphql"] and "pullRequests(" in query:
    nodes = [node(11, "Mine failing", "me", False, "FAILURE", False), node(12, "Asks me", "zed", False, "SUCCESS", True),
             node(13, "Draft work", "amy", True, "PENDING", False)]
    nodes = [n for n in nodes if not os.path.exists(gone + "-%d" % n["number"])]
    print(json.dumps({"data": {"repository": {"pullRequests": {"pageInfo": {"hasNextPage": False, "endCursor": None}, "nodes": nodes}}}}))
elif args[:1] == ["pr"] and args[1:2] in (["merge"], ["close"], ["ready"], ["checkout"]):
    pass
else:
    sys.stderr.write("fake gh: no answer for %s\n" % " ".join(shown))
    sys.exit(1)
PY
chmod +x "$T/bin/gh"
rm -f "$T/home/.zlogin"
{ [ -e "$HOME/.zlogin" ] && echo ". \"$HOME/.zlogin\""; echo "export PATH=\"$T/bin:\$PATH\""; } > "$T/home/.zlogin"

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

PATH="$T/bin:$PATH" HOME="$T/home" "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
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

# 11b the repo feed and its actions (wheel-pmf2.4.10): a fixture repo with an origin whose HEAD is
# main, a branch at main's tip (merged), a branch with a local-only commit, and one stash.
R="$T/proj-r"
ORIGIN="$T/origin.git"
git init -q --bare -b main "$ORIGIN"
git init -q -b main "$R"
rgit() { git -C "$R" -c user.name=t -c user.email=t@t -c commit.gpgsign=false "$@"; }
echo one > "$R/f.txt"
rgit add f.txt
rgit commit -q -m "first commit"
rgit remote add origin "$ORIGIN"
rgit push -q -u origin main
rgit remote set-head origin main > /dev/null
rgit branch merged-b
rgit checkout -q -b wip
echo two >> "$R/f.txt"
rgit commit -q -am "local-only work"
rgit checkout -q main
echo three >> "$R/f.txt"
rgit stash -q
cat > "$R/wheel.toml" <<TOML
[repo.list.mark]
label = "Mark branches"
run = 'echo "\$BRANCHES|\$REPO_PATH" > "$T/marker-repo-list"'
TOML
RSPACE=$(w space new "$R")
RPANE=$(w pane ls | awk -v s="$RSPACE" '$1 == s {print $2; exit}')
w pane kind "$RPANE" plugin --plugin wheelhouse --plugin-pane repo > /dev/null
FR="$T/feed-r.jsonl"
w plugin feed "$RPANE" wheelhouse.repo --params "{\"dir\":\"$R\"}" --count 200 > "$FR" 2> "$FR.err" &
PIDS+=($!)
poll 30 grep -q '"branches":\[{' "$FR"
python3 - "$FR" "$R" > "$T/repo-shape.verdict" <<'PY'
import json, sys
path, dirn = sys.argv[1:]
rows = [json.loads(l) for l in open(path) if l.strip().startswith("{")]
vals = [r for r in rows if r.get("value") and r["value"].get("branches")]
if not vals:
    print("no value with branches: %r" % rows[:2]); sys.exit()
r = vals[0]
v = r["value"]
want = {"now", "name", "top", "default", "head", "branches", "stashes", "commits", "pr_error", "error", "scripts"}
bkeys = {"branch", "ts", "subject", "author", "ahead", "behind", "upstream", "upstream_gone", "remote", "unpushed",
         "worktree", "pr", "pr_merged", "local_only", "dead", "blocked"}
by = {b["branch"]: b for b in v["branches"]}
problems = []
if r.get("params") != {"dir": dirn}:
    problems.append("params %r" % r.get("params"))
if set(v) != want:
    problems.append("keys %r" % sorted(v))
if v["name"] != "proj-r" or v["default"] != "origin/main" or v["head"] != "main" or v["error"] is not None:
    problems.append("header %r" % {k: v[k] for k in ("name", "default", "head", "error")})
if set(by) != {"merged-b", "wip"} or any(set(b) != bkeys for b in v["branches"]):
    problems.append("branches %r" % sorted(by))
elif not (by["merged-b"]["ahead"] == 0 and by["merged-b"]["dead"] == "merged" and by["merged-b"]["local_only"] == 0
          and by["wip"]["ahead"] == 1 and by["wip"]["dead"] is None and by["wip"]["local_only"] == 1):
    problems.append("branch fields %r" % by)
if len(v["stashes"]) != 1 or set(v["stashes"][0]) != {"ref", "subject", "when"} or v["stashes"][0]["ref"] != "stash@{0}":
    problems.append("stashes %r" % v["stashes"])
if [c["subject"] for c in v["commits"]] != ["first commit"] or set(v["commits"][0]) != {"sha", "subject", "author", "ts"}:
    problems.append("commits %r" % v["commits"])
if v["scripts"] != [{"name": "mark", "label": "Mark branches"}]:
    problems.append("scripts %r" % v["scripts"])
print("ok" if not problems else "; ".join(problems))
PY
check "the repo feed lists branches, stashes, commits and scripts with the documented keys" "$(head -1 "$T/repo-shape.verdict")"

# 2 a new branch arrives as a patch.
rgit branch new-x
has_new_x() { grep '"patch"' "$FR" | grep -q 'new-x'; }
poll 30 has_new_x
has_new_x && pass "git branch new-x arrives in the repo feed as a patch" || fail "git branch new-x arrives in the repo feed as a patch"

# 3 branch_delete.
w plugin action "$RPANE" wheelhouse.branch_delete --args "{\"dir\":\"$R\",\"branch\":\"merged-b\"}" > "$T/del.json" 2> "$T/del.err"
python3 - "$T/del.json" > "$T/del.verdict" <<'PY'
import json, sys
try:
    r = json.loads(open(sys.argv[1]).readline())
except Exception as e:
    print("no result: %s" % e); sys.exit()
print("ok" if "deleted branch merged-b" in r.get("said", "") else repr(r))
PY
check "branch_delete of a merged branch answers said with 'deleted branch'" "$(cat "$T/del.verdict")"
# The patches are checked by their text: one of them removes merged-b's row.
removed() { grep '"patch"' "$FR" | grep '"remove"' | grep -q '/branches/'; }
poll 30 removed
removed && [ -z "$(git -C "$R" branch --list merged-b)" ] && pass "the deleted branch is gone from git and leaves the repo feed (a remove patch)" \
  || fail "the deleted branch leaves the repo feed"
w plugin action "$RPANE" wheelhouse.branch_delete --args "{\"dir\":\"$R\",\"branch\":\"wip\"}" > /dev/null 2> "$T/del2.err"
grep -q 'wip: not deletable' "$T/del2.err" && ! grep -q 'wip: wip:' "$T/del2.err" && [ -n "$(git -C "$R" branch --list wip)" ] \
  && pass "branch_delete of a branch with local-only commits errors with 'not deletable' (the branch named once) and keeps it" \
  || fail "branch_delete of wip: $(cat "$T/del2.err")"

# 4 run_list with view repo runs [repo.list] with BRANCHES and REPO_PATH.
w plugin action "$RPANE" wheelhouse.run_list --args "{\"dir\":\"$R\",\"view\":\"repo\",\"script\":\"mark\",\"ids\":[\"wip\",\"new-x\"]}" > "$T/rlist.json" 2> "$T/rlist.err"
poll 5 has_file "$T/marker-repo-list"
RTOP=$(cd "$R" && pwd -P)
if [ "$(cat "$T/marker-repo-list" 2>/dev/null)" = "wip new-x|$RTOP" ] && grep -q 'Mark branches started over 2 branches' "$T/rlist.json"; then
  pass "run_list with view repo runs [repo.list] with BRANCHES and REPO_PATH"
else
  fail "run_list view repo: $(cat "$T/rlist.json") $(cat "$T/rlist.err") marker=$(cat "$T/marker-repo-list" 2>/dev/null)"
fi
w plugin action "$RPANE" wheelhouse.run_list --args "{\"dir\":\"$R\",\"view\":\"nope\",\"script\":\"mark\",\"ids\":[\"wip\"]}" > /dev/null 2> "$T/rview.err"
grep -q 'no view nope' "$T/rview.err" && pass "run_list with an unknown view is an error" || fail "run_list unknown view: $(cat "$T/rview.err")"

# 11c a repo feed on a checkout with no origin sees `git remote add origin` and the first fetch at once
# (the missing refs/remotes/origin is watched through its nearest existing ancestor), not at the 120 s tick.
N="$T/proj-n"
git init -q -b main "$N"
ngit() { git -C "$N" -c user.name=t -c user.email=t@t -c commit.gpgsign=false "$@"; }
echo n > "$N/f.txt"
ngit add f.txt
ngit commit -q -m "only commit"
FN="$T/feed-n.jsonl"
w plugin feed "$RPANE" wheelhouse.repo --params "{\"dir\":\"$N\"}" --count 200 > "$FN" 2> "$FN.err" &
PIDS+=($!)
poll 30 grep -q '"branches"' "$FN"
ngit remote add origin "$ORIGIN"
ngit fetch -q origin
saw_origin() { grep '"patch"' "$FN" | grep -q 'origin'; }
poll 3 saw_origin
saw_origin && pass "git remote add origin and the first fetch reach a repo feed that had no origin within 3 s" \
  || fail "git remote add origin and the first fetch reach the repo feed: $(tail -c 400 "$FN")"

# 11d closing the repo pane's feed on a directory leaves the beads feed on the same directory updating.
FRA="$T/feed-ra.jsonl"
w plugin feed "$RPANE" wheelhouse.repo --params "{\"dir\":\"$A\"}" --count 200 > "$FRA" 2> "$FRA.err" &
RAPID=$!
PIDS+=($RAPID)
poll 30 grep -q '"now"' "$FRA"
kill "$RAPID" 2>/dev/null
wait "$RAPID" 2>/dev/null
sleep 1
( cd "$A" && bd update "$C2" --title "After the repo feed closed" > /dev/null 2>&1 )
poll 10 grep -q 'After the repo feed closed' "$FA"
grep -q 'After the repo feed closed' "$FA" && pass "closing a repo feed on a directory leaves the beads feed on it updating" \
  || fail "closing a repo feed on a directory leaves the beads feed on it updating"

# 11e the diff feed (wheel-pmf2.4.11): three clean fixture repos, each with a .gitignore holding
# `target/` and a tracked src/nested.txt; the value has the documented keys, an edit and a new untracked
# file arrive as patches within 5 s, a write under the ignored target/ sends nothing while a write to a
# tracked file right after it does, the second repo's feed is untouched, and in the third repo (a beads
# feed opened on it first) an edit of src/nested.txt arrives within 5 s.
D1="$T/proj-d1"
D2="$T/proj-d2"
D3="$T/proj-d3"
for d in "$D1" "$D2" "$D3"; do
  git init -q -b main "$d"
  echo "target/" > "$d/.gitignore"
  echo "line one" > "$d/f.txt"
  mkdir -p "$d/src"
  echo "nested one" > "$d/src/nested.txt"
  git -C "$d" add .gitignore f.txt src/nested.txt
  git -C "$d" -c user.name=t -c user.email=t@t -c commit.gpgsign=false commit -q -m "first commit"
done
DSPACE=$(w space new "$D1")
DPANE=$(w pane ls | awk -v s="$DSPACE" '$1 == s {print $2; exit}')
w pane kind "$DPANE" plugin --plugin wheelhouse --plugin-pane diff > /dev/null
FD1="$T/feed-d1.jsonl"
FD2="$T/feed-d2.jsonl"
w plugin feed "$DPANE" wheelhouse.diff --params "{\"dir\":\"$D1\"}" --count 200 > "$FD1" 2> "$FD1.err" &
PIDS+=($!)
w plugin feed "$DPANE" wheelhouse.diff --params "{\"dir\":\"$D2\"}" --count 200 > "$FD2" 2> "$FD2.err" &
PIDS+=($!)
poll 30 grep -q '"scopes"' "$FD1"
poll 30 grep -q '"scopes"' "$FD2"
# diff_state.py <feed file>: the value after applying every patch row in order (add, remove, replace).
cat > "$T/diff_state.py" <<'PY'
import json, sys

def unescape(s):
    return s.replace("~1", "/").replace("~0", "~")

def walk(doc, parts):
    for p in parts:
        doc = doc[int(p)] if isinstance(doc, list) else doc[p]
    return doc

def apply(doc, op):
    parts = [unescape(p) for p in op["path"].split("/")[1:]]
    if not parts:
        return op["value"]
    parent, last = walk(doc, parts[:-1]), parts[-1]
    if isinstance(parent, list):
        i = len(parent) if last == "-" else int(last)
        if op["op"] == "add":
            parent.insert(i, op["value"])
        elif op["op"] == "remove":
            parent.pop(i)
        else:
            parent[i] = op["value"]
    elif op["op"] == "remove":
        del parent[last]
    else:
        parent[last] = op["value"]
    return doc

rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip().startswith("{")]
state, patches = None, 0
for r in rows:
    if r.get("stale"):
        continue
    if "value" in r and r["value"] is not None:
        state = r["value"]
    elif "patch" in r and state is not None:
        patches += 1
        for op in r["patch"]:
            state = apply(state, op)
files = [] if state is None else [[f["path"], f["status"], len(f["hunks"])] for f in state["scopes"][0]["files"]]
json.dump({"patches": patches, "files": files}, sys.stdout)
PY
diff_files() { python3 "$T/diff_state.py" "$1" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["files"]))'; }
diff_patches() { python3 "$T/diff_state.py" "$1" | python3 -c 'import json,sys; print(json.load(sys.stdin)["patches"])'; }
python3 - "$FD1" > "$T/diff-shape.verdict" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip().startswith("{")]
vals = [r for r in rows if r.get("value") and "scopes" in r["value"]]
if not vals:
    print("no value with scopes: %r" % rows[:2]); sys.exit()
v = vals[0]["value"]
want = {"name", "top", "branch", "head", "default", "error", "scopes"}
problems = []
if set(v) != want:
    problems.append("keys %r" % sorted(v))
if v["error"] is not None or v["name"] != "proj-d1" or v["branch"] != "main":
    problems.append("header %r" % {k: v[k] for k in ("name", "branch", "error")})
if not v["scopes"] or v["scopes"][0]["key"] != "uncommitted" or v["scopes"][0]["files"] != []:
    problems.append("scopes %r" % v["scopes"][:1])
print("ok" if not problems else "; ".join(problems))
PY
check "the diff feed has the documented keys and an empty uncommitted scope first" "$(head -1 "$T/diff-shape.verdict")"

# b an edit of a tracked file arrives as a patch.
echo "line two" >> "$D1/f.txt"
diff_has_edit() { [ "$(diff_files "$FD1")" = '[["f.txt", "M", 1]]' ]; }
poll 5 diff_has_edit
diff_has_edit && pass "an edit of a tracked file reaches the diff feed within 5 s with a hunk" \
  || fail "an edit of a tracked file reaches the diff feed: $(diff_files "$FD1")"

# c a new untracked file arrives as a patch.
echo "fresh" > "$D1/new.txt"
diff_has_new() { diff_files "$FD1" | grep -q '\["new.txt", "?"'; }
poll 5 diff_has_new
diff_has_new && pass "a new untracked file reaches the diff feed within 5 s with status ?" \
  || fail "a new untracked file reaches the diff feed: $(diff_files "$FD1")"

# d a write under the ignored target/ sends nothing; the watch is live, since a write to a tracked
# file right after it arrives, and only that file's change does.
BEFORE=$(diff_patches "$FD1")
mkdir -p "$D1/target"
echo built > "$D1/target/out.bin"
sleep 1
echo more >> "$D1/target/out.bin"
sleep 2
AFTER=$(diff_patches "$FD1")
[ "$BEFORE" = "$AFTER" ] && pass "writing a file under the git-ignored target/ sends no update within 3 s" \
  || fail "a write under target/ sent updates ($BEFORE patches before, $AFTER after)"
echo "line three" >> "$D1/f.txt"
diff_after_target() { [ "$(diff_patches "$FD1")" -gt "$AFTER" ]; }
poll 5 diff_after_target
diff_after_target && ! diff_files "$FD1" | grep -q 'target/' \
  && pass "a tracked file's write right after the target/ writes arrives, and nothing from target/ does" \
  || fail "the watch is not live after the target/ writes: $(diff_files "$FD1")"

# e the second repo's feed got no update through all of it.
[ "$(diff_patches "$FD2")" = 0 ] && [ "$(diff_files "$FD2")" = '[]' ] && pass "the second project's diff feed is unchanged" \
  || fail "the second project's diff feed changed: $(diff_patches "$FD2") patches, files $(diff_files "$FD2")"

# f a beads feed on the checkout opened first leaves the diff feed's recursive watch in place: an edit of
# a nested tracked file arrives.
FB3="$T/feed-b3.jsonl"
FD3="$T/feed-d3.jsonl"
w plugin feed "$PANE" wheelhouse.beads --params "{\"dir\":\"$D3\"}" --count 200 > "$FB3" 2> "$FB3.err" &
PIDS+=($!)
poll 30 test -s "$FB3"
w plugin feed "$DPANE" wheelhouse.diff --params "{\"dir\":\"$D3\"}" --count 200 > "$FD3" 2> "$FD3.err" &
PIDS+=($!)
poll 30 grep -q '"scopes"' "$FD3"
echo "nested two" >> "$D3/src/nested.txt"
diff_nested() { [ "$(diff_files "$FD3")" = '[["src/nested.txt", "M", 1]]' ]; }
poll 5 diff_nested
diff_nested && pass "an edit of a nested tracked file reaches the diff feed within 5 s when a beads feed opened the checkout first" \
  || fail "a nested edit does not reach the diff feed after a beads feed: $(diff_files "$FD3")"

# 11f the PRs pane (wheel-pmf2.4.12): a pane of kind wheelhouse/prs reads `wheelhouse.prs` and
# `wheelhouse.pr_detail` of a temp project through the fake gh (three canned PRs), and invokes the PR actions.
# Every gh call is logged to $T/gh.log.
PP="$T/proj-p"
git init -q -b main "$PP"
cat > "$PP/wheel.toml" <<TOML
[prs.list.mark]
label = "Mark PRs"
run = 'echo "\$PR_NUMBERS" > "$T/marker-prs-list"'
TOML
PSPACE=$(w space new "$PP")
PPANE=$(w pane ls | awk -v s="$PSPACE" '$1 == s {print $2; exit}')
w pane kind "$PPANE" plugin --plugin wheelhouse --plugin-pane prs > /dev/null

# 6 the pane's script is part of the package: the pane is listed and its manifest entry names the script.
w plugin ls --space "$PSPACE" > "$T/pls.txt" 2>&1
if grep -q 'prs' "$T/pls.txt" && [ -s "$ROOT/plugins/wheelhouse/panes/prs.luau" ] && ! grep -qi 'error' "$T/pls.txt"; then
  pass "a pane of kind wheelhouse/prs is listed with its script and no error"
else
  fail "wheelhouse/prs pane listing: $(cat "$T/pls.txt")"
fi

# 1 the prs feed.
w plugin feed "$PPANE" wheelhouse.prs --params "{\"dir\":\"$PP\"}" --count 200 > "$T/prs-feed.json" 2> "$T/prs-feed.err" &
PIDS+=($!)
poll 30 grep -q '"prs":\[{' "$T/prs-feed.json"
python3 - "$T/prs-feed.json" > "$T/prs-shape.verdict" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip().startswith("{")]
vals = [r["value"] for r in rows if r.get("value")]
if not vals:
    print("no value: %r" % rows[:2]); sys.exit()
v = vals[0]
problems = []
want = {"now", "name", "repo", "me", "error", "fetched", "fetched_clock", "next_clock", "prs", "buttons", "scripts"}
if not want <= set(v):
    problems.append("missing keys %r" % sorted(want - set(v)))
by = {p["number"]: p for p in v.get("prs", [])}
if set(by) != {11, 12, 13}:
    problems.append("numbers %r" % sorted(by))
else:
    got = {n: (p["bucket"], p["standing"]) for n, p in by.items()}
    exp = {11: ("mine", "no-reviewer"), 12: ("needs-me", "review"), 13: ("other", "not-asked")}
    if got != exp:
        problems.append("buckets and standings %r" % got)
    if not (by[11]["checks"] == "failing" and by[11]["fix"] is True and by[13]["draft"] is True and by[12]["checks"] == "passing"):
        problems.append("checks, fix or draft %r" % {n: (p["checks"], p["fix"], p["draft"]) for n, p in by.items()})
    pkeys = {"number", "title", "author", "branch", "draft", "mergeable", "checks", "review", "bucket", "my", "standing", "updated", "url",
             "fix", "additions", "deletions", "files", "commits", "labels", "reviewers", "summary"}
    if not pkeys <= set(by[11]):
        problems.append("pr keys missing %r" % sorted(pkeys - set(by[11])))
if v.get("repo") != "o/fixture" or v.get("me") != "me" or v.get("error") is not None or not v.get("fetched"):
    problems.append("header %r" % {k: v.get(k) for k in ("repo", "me", "error", "fetched")})
if v.get("scripts") != [{"name": "mark", "label": "Mark PRs"}]:
    problems.append("scripts %r" % v.get("scripts"))
print("ok" if not problems else "; ".join(problems))
PY
check "the prs feed lists the 3 PRs with their buckets (mine, needs-me, other), standings and the list scripts" "$(head -1 "$T/prs-shape.verdict")"

# 2 the pr_detail feed.
w plugin feed "$PPANE" wheelhouse.pr_detail --params "{\"dir\":\"$PP\",\"number\":11}" --count 200 > "$T/prd-feed.json" 2> "$T/prd-feed.err" &
PIDS+=($!)
poll 30 grep -q '"body":\[{' "$T/prd-feed.json"
python3 - "$T/prd-feed.json" > "$T/prd-shape.verdict" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip().startswith("{")]
vals = [r["value"] for r in rows if r.get("value")]
if not vals:
    print("no value: %r" % rows[:2]); sys.exit()
v = vals[0]
problems = []
for k in ("now", "number", "error", "state", "draft", "mergeable", "merge_state", "base", "created", "updated", "url", "assignees", "reviewers",
          "labels", "body", "checks", "talk", "commits_total", "commits", "files_total", "files", "agents"):
    if k not in v:
        problems.append("missing " + k)
if not problems:
    kinds = {b["kind"] for b in v["body"]}
    if not {"text", "image"} <= kinds:
        problems.append("body kinds %r" % sorted(kinds))
    if not any(b["kind"] == "image" and b["url"] == "https://example.com/shot.png" and b["alt"] == "shot" for b in v["body"]):
        problems.append("no image block for the shot %r" % v["body"])
    if not any(b["kind"] == "text" and b.get("heading") == 2 and "Plan" in b["text"] for b in v["body"]):
        problems.append("no H2 text block %r" % v["body"])
    if v["number"] != 11 or v["state"] != "OPEN" or v["base"] != "main" or v["error"] is not None:
        problems.append("header %r" % {k: v[k] for k in ("number", "state", "base", "error")})
    if [c["name"] for c in v["checks"]] != ["build"] or v["checks"][0]["state"] != "SUCCESS":
        problems.append("checks %r" % v["checks"])
    if len(v["talk"]) != 1 or v["talk"][0]["who"] != "zed" or not v["talk"][0]["blocks"]:
        problems.append("talk %r" % v["talk"])
    if v["commits"][0]["sha"] != "abc1234" or v["files"][0]["path"] != "src/a.rs":
        problems.append("commits or files %r %r" % (v["commits"], v["files"]))
print("ok" if not problems else "; ".join(problems))
PY
check "the pr_detail feed carries the body as text and image blocks, the check, the conversation, commits and files" "$(head -1 "$T/prd-shape.verdict")"

# 3a two pr_detail subscriptions of one directory (numbers 11 and 12) each carry their own PR and each get an update after an
# action; pr_refresh answers and asks gh for the list again.
w plugin feed "$PPANE" wheelhouse.pr_detail --params "{\"dir\":\"$PP\",\"number\":12}" --count 200 > "$T/prd12-feed.json" 2> "$T/prd12-feed.err" &
PIDS+=($!)
w plugin feed "$PPANE" wheelhouse.pr_detail --params "{\"dir\":\"$PP\",\"number\":11}" --count 200 > "$T/prd11-feed.json" 2> "$T/prd11-feed.err" &
PIDS+=($!)
poll 30 grep -q '"number":12' "$T/prd12-feed.json"
poll 30 grep -q '"number":11' "$T/prd11-feed.json"
first_number() { python3 -c '
import json, sys
for l in open(sys.argv[1]):
    if l.strip().startswith("{") and json.loads(l).get("value"):
        print(json.loads(l)["value"]["number"]); break' "$1"; }
if [ "$(first_number "$T/prd11-feed.json")" = 11 ] && [ "$(first_number "$T/prd12-feed.json")" = 12 ]; then
  pass "two pr_detail subscriptions of one directory each receive their own PR (11 and 12)"
else
  fail "pr_detail numbers: 11 -> $(first_number "$T/prd11-feed.json"), 12 -> $(first_number "$T/prd12-feed.json")"
fi
more_rows() { [ "$(rows "$1")" -gt "$2" ]; }
R11=$(rows "$T/prd11-feed.json"); R12=$(rows "$T/prd12-feed.json")
w plugin action "$PPANE" wheelhouse.pr_draft --args "{\"dir\":\"$PP\",\"number\":12,\"draft\":true}" > /dev/null 2>&1
if poll 10 more_rows "$T/prd11-feed.json" "$R11" && poll 10 more_rows "$T/prd12-feed.json" "$R12"; then
  pass "after an action both pr_detail subscriptions (11 and 12) receive an update"
else
  fail "pr_detail updates after pr_draft: 11 rows $R11 -> $(rows "$T/prd11-feed.json"), 12 rows $R12 -> $(rows "$T/prd12-feed.json")"
fi
lists() { grep -c '^api graphql list$' "$T/gh.log"; }
more_lists() { [ "$(lists)" -gt "$1" ]; }
L0=$(lists)
if w plugin action "$PPANE" wheelhouse.pr_refresh --args "{\"dir\":\"$PP\"}" > "$T/prref.json" 2> "$T/prref.err" && poll 10 more_lists "$L0"; then
  pass "pr_refresh answers and asks gh for the list again ($L0 to $(lists) list queries)"
else
  fail "pr_refresh: $(cat "$T/prref.json") $(cat "$T/prref.err") list queries $L0 -> $(lists)"
fi
if ! w plugin action "$PPANE" wheelhouse.pr_close --args "{\"dir\":\"$PP\",\"number\":99}" > "$T/pr99.json" 2> "$T/pr99.err" \
   && grep -q 'no open PR' "$T/pr99.json" "$T/pr99.err" && ! grep -qx 'pr close 99' "$T/gh.log"; then
  pass "an action with a number not in the list fails with 'no open PR' and runs no gh pr command"
else
  fail "pr_close 99: $(cat "$T/pr99.json") $(cat "$T/pr99.err")"
fi

# 3 pr_merge.
PRS_ROWS=$(rows "$T/prs-feed.json"); L1=$(lists)
w plugin action "$PPANE" wheelhouse.pr_merge --args "{\"dir\":\"$PP\",\"number\":11,\"method\":\"squash\"}" > "$T/prm.json" 2> "$T/prm.err"
if grep -q 'Merged #11 (squash).' "$T/prm.json" && grep -qx 'pr merge 11 --squash' "$T/gh.log"; then
  pass "pr_merge answers said 'Merged #11 (squash).' and runs gh pr merge 11 --squash"
else
  fail "pr_merge: $(cat "$T/prm.json") $(cat "$T/prm.err") gh: $(grep 'pr ' "$T/gh.log")"
fi

if poll 10 more_rows "$T/prs-feed.json" "$PRS_ROWS" && poll 10 more_lists "$L1"; then
  pass "after pr_merge the prs feed sends a later update and gh is asked for the list again"
else
  fail "after pr_merge: prs rows $PRS_ROWS -> $(rows "$T/prs-feed.json"), list queries $L1 -> $(lists)"
fi

# 4 pr_draft and pr_close.
w plugin action "$PPANE" wheelhouse.pr_draft --args "{\"dir\":\"$PP\",\"number\":12,\"draft\":true}" > "$T/prdr.json" 2> "$T/prdr.err"
w plugin action "$PPANE" wheelhouse.pr_draft --args "{\"dir\":\"$PP\",\"number\":13,\"draft\":false}" > "$T/prdr2.json" 2> "$T/prdr2.err"
if grep -q '"said"' "$T/prdr.json" && grep -q '"said"' "$T/prdr2.json" && grep -qx 'pr ready 12 --undo' "$T/gh.log" && grep -qx 'pr ready 13' "$T/gh.log"; then
  pass "pr_draft runs gh pr ready 12 --undo to convert to draft and gh pr ready 13 to mark ready"
else
  fail "pr_draft: $(cat "$T/prdr.json") $(cat "$T/prdr.err") $(cat "$T/prdr2.json") gh: $(grep 'pr ' "$T/gh.log")"
fi
w plugin action "$PPANE" wheelhouse.pr_close --args "{\"dir\":\"$PP\",\"number\":12}" > "$T/prc.json" 2> "$T/prc.err"
if grep -q '"said"' "$T/prc.json" && grep -qx 'pr close 12' "$T/gh.log"; then
  pass "pr_close answers said and runs gh pr close 12"
else
  fail "pr_close: $(cat "$T/prc.json") $(cat "$T/prc.err") gh: $(grep 'pr ' "$T/gh.log")"
fi

# 5 run_list with view prs runs [prs.list] with PR_NUMBERS.
w plugin action "$PPANE" wheelhouse.run_list --args "{\"dir\":\"$PP\",\"view\":\"prs\",\"script\":\"mark\",\"ids\":[\"11\",\"12\"]}" > "$T/prl.json" 2> "$T/prl.err"
poll 5 has_file "$T/marker-prs-list"
if [ "$(cat "$T/marker-prs-list" 2>/dev/null)" = "11 12" ] && grep -q 'Mark PRs started over 2 PRs' "$T/prl.json"; then
  pass "run_list with view prs runs [prs.list] with PR_NUMBERS"
else
  fail "run_list view prs: $(cat "$T/prl.json") $(cat "$T/prl.err") marker=$(cat "$T/marker-prs-list" 2>/dev/null)"
fi

# 12 kill -9 of the backend: stale, then a fresh value.
OLD=$(backend_pid "$SPACE")
kill -9 "$OLD" 2>/dev/null
poll 20 backend_pid_not "$SPACE" "$OLD"
NEW=$(backend_pid "$SPACE")
[ -n "$NEW" ] && [ "$NEW" != "$OLD" ] && pass "wheeld restarts the backend after kill -9 ($OLD to $NEW)" || fail "backend restart ($OLD, now '$NEW')"
stale_verdict() {
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
  grep -qx ok "$T/stale.verdict"
}
# The new process runs bd before it answers the replayed subscribe; wait for its value.
poll 15 stale_verdict
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
