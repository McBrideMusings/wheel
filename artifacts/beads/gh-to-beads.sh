#!/bin/sh
# Turns `gh issue list --state all --json number,title,state,labels,body,createdAt,closedAt` (stdin) into the
# document the beads page reads as data/beads.json, one repo named by $1 (default: the current directory's name).
# $2 is optional: the output of `gh api graphql --paginate` over issues { number parent { number }
# blockedBy { nodes { number state } } }, which gives Features its grouping and the blocked state.
# id = "#<number>"; type = first label among bug|feature|task|epic|chore|decision, else task;
# priority = a P0..P4 label, else 2.
exec python3 -c '
import json, sys
name, rel_path = sys.argv[1], (sys.argv[2] if len(sys.argv) > 2 else None)
issues = json.load(sys.stdin)
rel = {}
if rel_path:
    text, dec, i = open(rel_path).read(), json.JSONDecoder(), 0
    while True:
        while i < len(text) and text[i].isspace(): i += 1
        if i >= len(text): break
        page, i = dec.raw_decode(text, i)
        for n in page["data"]["repository"]["issues"]["nodes"]:
            rel[n["number"]] = n
TYPES = ("bug", "feature", "task", "epic", "chore", "decision")
state = {i["number"]: i["state"].lower() for i in issues}
def labels(i): return [l["name"] if isinstance(l, dict) else l for l in i.get("labels") or []]
def kind(ls): return next((l.lower() for l in ls if l.lower() in TYPES), "task")
def prio(ls): return next((int(l[1]) for l in ls if len(l) == 2 and l[0] in "pP" and l[1] in "01234"), 2)
parents = {i["number"]: (rel.get(i["number"], {}).get("parent") or {}).get("number") for i in issues}
kids = {p for p in parents.values() if p}
beads = []
for i in issues:
    n, ls = i["number"], labels(i)
    closed = i["state"].upper() == "CLOSED"
    blockers = [b["number"] for b in (rel.get(n, {}).get("blockedBy") or {}).get("nodes", []) if b["state"].upper() == "OPEN"]
    beads.append(dict(id="#%d" % n, title=i["title"], status="closed" if closed else "open", type=kind(ls),
        priority=prio(ls), parent=("#%d" % parents[n]) if parents[n] in state else None, labels=ls, owner=None,
        updated=i.get("closedAt") or i.get("createdAt"), blocked_by=["#%d" % b for b in blockers],
        description=(i.get("body") or "")[:1500], ready=not closed and not blockers,
        epic=kind(ls) == "epic" or n in kids))
json.dump({"repos": [dict(name=name, path="", scripts=[], beads=beads, error=None)], "problems": []}, sys.stdout, indent=1)
' "${1:-$(basename "$PWD")}" ${2:+"$2"}
