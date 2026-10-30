#!/usr/bin/env bash
# § Project artifact (wheel-z3b7): artifacts/project/collect.py against a fixture beads repo, a
# folder with no tracker and one whose tracker is broken, and (when it is there) this repo. Needs
# bd and /usr/bin/python3; no daemon, no app. Prints one ok / FAIL line per check, then the
# snapshot-size measure perf.md § project limits. The page itself is checked in the Mac app
# (verify.md § 16).
set -u
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
COLLECT="$REPO/artifacts/project/collect.py"
SCR="$(mktemp -d "${TMPDIR:-/tmp}/wheel-project.XXXXXX")"
SCR="$(cd "$SCR" && pwd -P)"
trap 'rm -rf "$SCR"' EXIT
command -v bd >/dev/null || { echo "these checks need bd" >&2; exit 2; }
PASS=0 FAIL=0
ok() { PASS=$((PASS + 1)); echo "ok   $1"; }
fail() { FAIL=$((FAIL + 1)); echo "FAIL $1"; }
# check <name> <python expression over d, the parsed JSON on stdin>
check() { if python3 -c "import json,sys; d=json.load(sys.stdin); sys.exit(0 if ($2) else 1)" 2>/dev/null; then ok "$1"; else fail "$1"; fi; }
collect() { (cd "$1" && env -u WHEEL_PANE -u WHEEL_SPACE_DIR /usr/bin/python3 "$COLLECT" "${@:2}" 2>"$SCR/stderr"); }

# The fixture: a P1 epic with two child epics (features) and a bead beside them, an epic with only
# beads, an empty epic, a deferred epic, open and closed P1 bugs and chores, a blocks edge, a human
# bead, an image, a table, design / acceptance / notes and two comments.
F="$SCR/fixture"
mkdir -p "$F/docs" "$F/shots"
(
  cd "$F" && git init -q
  printf '# Fixture\n\nThe vision paragraph,\nover two lines.\n\n## Language\n\nNot the vision.\n' > docs/CONTEXT.md
  printf '\x89PNG\r\n\x1a\n' > shots/dot.png
  bd init --prefix fx --non-interactive >/dev/null 2>&1
  id() { python3 -c 'import json,sys;d=json.load(sys.stdin);print((d[0] if isinstance(d,list) else d)["id"])'; }
  A=$(bd create --type epic -p 1 --title "Player sheet" -d "Destination." --json | id)
  F1=$(bd create --type epic -p 2 --parent "$A" --title "Dice Tray" --json | id)
  bd create --type epic -p 2 --parent "$A" --title "HP control" --json >/dev/null
  B1=$(bd create --type feature -p 2 --parent "$F1" --title "Tray UI" -d "![tray](shots/dot.png)

| a | b |
|---|---|
| 1 | 2 |" --design "Design text" --acceptance "Acceptance text" --notes "Notes text" --json | id)
  bd create --type decision -p 2 --parent "$F1" --title "Dice feel" -l human --json >/dev/null
  bd create --type feature -p 2 --parent "$F1" --title "Pad" --deps "blocked-by:$B1" --json >/dev/null
  bd create --type task -p 3 --parent "$A" --title "Beside the features" --json >/dev/null
  C=$(bd create --type epic -p 2 --title "DM authoring" --json | id)
  C1=$(bd create --type feature -p 2 --parent "$C" --title "Shipped thing" -d "A long closed description" --json | id)
  bd close "$C1" >/dev/null
  bd create --type epic -p 3 --title "Empty epic" --json >/dev/null
  E=$(bd create --type epic -p 1 --title "Combat" --json | id)
  bd update "$E" --status deferred >/dev/null
  bd create --type bug -p 1 --title "Open P1 bug" --json >/dev/null
  bd create --type chore -p 1 --title "Open P1 chore" --json >/dev/null
  G=$(bd create --type bug -p 1 --title "Closed P1 bug" --json | id)
  bd close "$G" >/dev/null
  bd create --type bug -p 2 --title "P2 bug" --json >/dev/null
  bd comments add "$B1" "First comment" >/dev/null
  bd comments add "$B1" "Second comment" >/dev/null
  echo "$B1" > "$SCR/b1"
) || { echo "the fixture failed to build" >&2; exit 2; }
B1=$(cat "$SCR/b1")

collect "$F" > "$SCR/fx.json"
check "fixture: state ok, named after the folder" 'd["state"] == "ok" and d["name"] == "fixture"' < "$SCR/fx.json"
check "fixture: vision is the first paragraph after the title" 'd["vision"] == "The vision paragraph, over two lines."' < "$SCR/fx.json"
check "fixture: every bead is in the snapshot" 'len(d["beads"]) == 15' < "$SCR/fx.json"
check "fixture: ready comes from bd ready" 'isinstance(d["ready"], list) and len(d["ready"]) > 0' < "$SCR/fx.json"
check "fixture: a closed bead travels without its description" 'all("description" not in b for b in d["beads"] if b["status"] == "closed")' < "$SCR/fx.json"
check "fixture: an open bead keeps description, design, acceptance and notes" '[b for b in d["beads"] if b["title"] == "Tray UI" and b["design"] == "Design text" and b["acceptance_criteria"] == "Acceptance text" and b["notes"] == "Notes text"]' < "$SCR/fx.json"
check "fixture: the snapshot keeps a local image's path" '"](shots/dot.png)" in [b for b in d["beads"] if b["title"] == "Tray UI"][0]["description"]' < "$SCR/fx.json"
check "fixture: parent-child edges are left out, blocks kept" '[b for b in d["beads"] if b["title"] == "Pad"][0]["deps"][0]["type"] == "blocks"' < "$SCR/fx.json"
check "fixture: the parent field names each child epic's epic" 'len([b for b in d["beads"] if b["type"] == "epic" and b["parent"]]) == 2' < "$SCR/fx.json"
check "fixture: a digest is set" 'len(d["digest"]) == 40' < "$SCR/fx.json"
grep -q "fixture: 15 beads, bd list" "$SCR/stderr" && ok "fixture: stderr names the repo, the count and bd's time" || fail "fixture: stderr line ($(cat "$SCR/stderr"))"

collect "$F" --bead "$B1" > "$SCR/b1.json"
check "--bead: the record with both comments" 'd["record"]["id"] == d["id"] and len(d["record"]["comments"]) == 2' < "$SCR/b1.json"
check "--bead: a local image becomes a data: URL" '"](data:image/png;base64," in d["record"]["description"]' < "$SCR/b1.json"
mkdir -p "$SCR/data/runs"
for i in $(seq 1 25); do : > "$SCR/data/runs/$i.log"; touch -t "202601010000.$(printf %02d $i)" "$SCR/data/runs/$i.log"; done
(cd "$F" && WHEEL_DATA_DIR="$SCR/data" /usr/bin/python3 "$COLLECT" --bead "$B1" >/dev/null 2>&1)
[ "$(ls "$SCR/data/runs" | wc -l | tr -d ' ')" = 20 ] && [ ! -e "$SCR/data/runs/1.log" ] && ok "--bead: keeps the newest 20 run logs" || fail "--bead: runs/ holds $(ls "$SCR/data/runs" | wc -l) logs"
[ "$(wc -l < "$SCR/b1.json" | tr -d ' ')" = 1 ] && ok "--bead: exactly one line" || fail "--bead: $(wc -l < "$SCR/b1.json") lines"
collect "$F" --bead 'x;rm -rf /' > "$SCR/bad.json"
check "--bead: a non-id is refused, never run" '"not a bead id" in d["error"]' < "$SCR/bad.json"
collect "$F" --bead fx-nope > "$SCR/nope.json"
check "--bead: a missing bead is bd's error" 'd["error"].startswith("bd show exit")' < "$SCR/nope.json"

mkdir -p "$SCR/plain"
collect "$SCR/plain" > "$SCR/plain.json"; code=$?
check "plain folder: state no-beads" 'd["state"] == "no-beads" and "beads" not in d' < "$SCR/plain.json"
[ "$code" = 0 ] && ok "plain folder: exit 0" || fail "plain folder: exit $code"

mkdir -p "$SCR/broken/.beads"
collect "$SCR/broken" > "$SCR/broken.json"; code=$?
check "broken tracker: state error with bd's Error line, repo still named" 'd["state"] == "error" and "no beads database found" in d["error"] and d["name"] == "broken"' < "$SCR/broken.json"
[ "$code" = 1 ] && ok "broken tracker: exit 1" || fail "broken tracker: exit $code"

git -C "$F" worktree add -q "$SCR/wt" 2>/dev/null && {
  collect "$SCR/wt" > "$SCR/wt.json"
  check "a worktree resolves to its main checkout" "d['repo'] == '$F'" < "$SCR/wt.json"
}

# perf.md § project: the snapshot this repo pushes every 30 s.
if [ -d "$REPO/.beads" ]; then
  t0=$(perl -MTime::HiRes=time -e 'print time')
  collect "$REPO" > "$SCR/repo.json"
  t1=$(perl -MTime::HiRes=time -e 'print time')
  kb=$(( $(wc -c < "$SCR/repo.json") / 1024 ))
  n=$(python3 -c "import json;print(len(json.load(open('$SCR/repo.json'))['beads']))")
  secs=$(perl -e "printf '%.1f', $t1 - $t0")
  echo "measure: this repo, $n beads: snapshot ${kb} KB (limit 800), collect ${secs} s (limit 10)"
  [ "$kb" -le 800 ] && ok "snapshot size under 800 KB" || fail "snapshot size ${kb} KB over 800"
  perl -e "exit($secs <= 10 ? 0 : 1)" && ok "collect under 10 s" || fail "collect took ${secs} s"
fi

echo "$PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
