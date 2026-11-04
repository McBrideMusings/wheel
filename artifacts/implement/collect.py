"""Refresh command for the implement artifact: pushes one JSON snapshot of the implement pass in the
space the pane sits in to the pane with `wheel artifact data "$WHEEL_PANE" -`, so the page redraws
without reloading. The snapshot is the one documented at the top of index.html. Run outside a pane
(no $WHEEL_PANE) it prints the snapshot instead.

Which pass: the space's directory ($WHEEL_SPACE_DIR, else the cwd). Its git branch gives the commits
ahead of the default branch and the diff against the merge base; its newest Claude transcript gives
the steps, the attempts and the checks. No pass (a directory on the default branch whose session never
declared a stage, or no git and no transcript) pushes {state: "none"}.

Sources, all read-only:
  git                                    branch, commits ahead, `diff --numstat` against the merge base
  <config dir>/projects/<cwd>/<id>.jsonl the session: `work_set` calls (stage and note, from the whole
                                         file by byte search) and Bash/Edit/Write/Agent calls with their
                                         results (the last TAIL_BYTES of the file)
  bd show <id> --json / gh pr view       the item's title and description, cached between refreshes
  ~/.local/state/work-board/sessions/    the item, when the transcript carries no work_set

The transcript parsing is the swarm collector's (`walk`, `transcript`, `short_command`, ...), imported
from artifacts/swarm/collect.py. $IMPLEMENT_TRANSCRIPT names a transcript to read instead of the
newest one for the directory (for testing a collector against a past pass).
"""
import glob
import hashlib
import json
import os
import re
import subprocess
import sys
import time
import importlib.util

# Both collectors are called collect.py, so the swarm one is loaded by path.
_spec = importlib.util.spec_from_file_location("swarm_collect", "/Users/pierce/Projects/wheel/artifacts/swarm/collect.py")
swarm = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(swarm)

HOME = os.path.expanduser("~")
TAIL_BYTES = 8_000_000   # how much of a transcript's end is parsed for tool calls
OUT_LINES = 40           # lines of a check's output kept
OUT_CHARS = 20_000       # characters of one attempt's output kept
OLD_OUT_CHARS = 2_000    # ... for attempts older than the newest FULL_OUT
FULL_OUT = 60
MAX_ATTEMPTS = 1500      # newest attempts kept
STEPS = [("plan", "Plan"), ("edit", "Edit"), ("build", "Build"), ("verify", "Verify"),
         ("review", "Review"), ("gate", "Gate")]
KEYS = [k for k, _ in STEPS]
CACHE = os.path.join(os.environ.get("TMPDIR", "/tmp"), "wheel-implement-items.json")

CHECK = re.compile(
    r"^(?:heavy\s+)?(?:"
    r"(?:bun|npm|pnpm|yarn)\s+(?:run\s+)?(?:\S+:)?(?:check|test|typecheck|tsc|lint|build|e2e|verify)\S*"
    r"|(?:bun|npx|pnpm\s+exec|yarn)\s+(?:test|tsc|vitest|jest|eslint|playwright|biome|oxlint)\b"
    r"|(?:\S*/)?(?:tsc|vitest|jest|eslint|playwright|pytest|biome|oxlint|xcodebuild|ruff|mypy)\b"
    r"|cargo\s+(?:test|build|check|clippy)\b|go\s+(?:test|build|vet)\b|swift\s+(?:build|test)\b"
    r"|make\b|python3?\s+-m\s+(?:pytest|unittest)\b|admin\s+(?:verify|check|test|build|lint|typecheck|e2e|screenshot|shot)\b"
    r")")
EDITS = ("Edit", "Write", "NotebookEdit", "MultiEdit")


def sh(args, cwd=None, timeout=15):
    code, out, _ = swarm.sh(args, cwd=cwd, timeout=timeout)
    return out if code == 0 else None


def git(d, *args):
    out = sh(["git", "-C", d, *args])
    return out.strip() if out is not None else None


def sid(x):
    return hashlib.sha1(str(x).encode()).hexdigest()[:8]


# ---------------------------------------------------------------- git

def default_branch(d):
    ref = git(d, "symbolic-ref", "--short", "refs/remotes/origin/HEAD")
    if ref:
        return ref.split("/", 1)[-1]
    for b in ("main", "master"):
        if git(d, "rev-parse", "--verify", "--quiet", "refs/heads/" + b):
            return b
    return None


def git_state(d):
    """branch, default branch, commits ahead and the diff against the merge base; None outside git."""
    top = git(d, "rev-parse", "--show-toplevel")
    if not top:
        return None
    branch = git(top, "symbolic-ref", "--short", "-q", "HEAD") or ("detached " + (git(top, "rev-parse", "--short", "HEAD") or ""))
    dflt = default_branch(top)
    g = {"top": top, "branch": branch, "default": dflt, "commits": [], "files": [], "base": None}
    if not dflt:
        return g
    base = next((r for r in ("origin/" + dflt, dflt) if git(top, "rev-parse", "--verify", "--quiet", r)), None)
    mb = git(top, "merge-base", "HEAD", base) if base else None
    g["base"] = mb
    if not mb:
        return g
    log = sh(["git", "-C", top, "log", mb + "..HEAD", "-n", "300", "--format=%h%x09%ct%x09%s"]) or ""
    for line in log.splitlines():
        p = line.split("\t", 2)
        if len(p) == 3 and p[1].isdigit():
            g["commits"].append({"sha": p[0], "subject": p[2], "ms": int(p[1]) * 1000})
    num = sh(["git", "-C", top, "diff", "--numstat", mb]) or ""
    for line in num.splitlines():
        p = line.split("\t", 2)
        if len(p) == 3:
            g["files"].append({"path": p[2], "add": int(p[0]) if p[0].isdigit() else 0, "del": int(p[1]) if p[1].isdigit() else 0})
    seen = {f["path"] for f in g["files"]}
    new = (sh(["git", "-C", top, "ls-files", "--others", "--exclude-standard", "-z"]) or "").split("\0")
    for path in [p for p in new if p and p not in seen][:60]:
        try:
            fp = os.path.join(top, path)
            n = 0
            if os.path.getsize(fp) < 1_000_000:
                with open(fp, "rb") as fh:
                    n = fh.read().count(b"\n")
            g["files"].append({"path": path, "add": n, "del": 0, "note": "untracked"})
        except OSError:
            pass
    return g


# ---------------------------------------------------------------- transcript

def newest_transcript(d):
    """The newest transcript Claude wrote for directory d, across config dirs (the dirs may be one
    symlinked folder, so each is counted once)."""
    enc = re.sub(r"[^A-Za-z0-9]", "-", d)
    best, seen = None, set()
    for c in swarm.config_dirs():
        pdir = os.path.realpath(os.path.join(c, "projects"))
        if pdir in seen:
            continue
        seen.add(pdir)
        for f in glob.glob(os.path.join(pdir, enc, "*.jsonl")):
            try:
                m = os.path.getmtime(f)
            except OSError:
                continue
            if best is None or m > best[0]:
                best = (m, f)
    return best[1] if best else None


def work_set_calls(path):
    """Every work_set tool call in the file, oldest first, found by byte search so the whole file is
    never parsed: [{id, ms, items:[{kind,id,stage,note}]}]."""
    with open(path, "rb") as fh:
        data = fh.read()
    calls, seen, pos = [], set(), 0
    while True:
        i = data.find(b"work_set", pos)
        if i < 0:
            break
        s = data.rfind(b"\n", 0, i) + 1
        e = data.find(b"\n", i)
        e = len(data) if e < 0 else e
        pos = e + 1
        line = data[s:e]
        if b'"tool_use"' not in line or b'"items"' not in line:
            continue
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        msg = ev.get("message") if isinstance(ev, dict) else None
        for c in (msg.get("content") if isinstance(msg, dict) and isinstance(msg.get("content"), list) else []):
            if isinstance(c, dict) and c.get("type") == "tool_use" and str(c.get("name", "")).endswith("work_set") \
                    and c.get("id") not in seen and isinstance(c.get("input"), dict):
                seen.add(c.get("id"))
                items = [i for i in c["input"].get("items") or [] if isinstance(i, dict)]
                calls.append({"id": c.get("id"), "ms": swarm.iso_ms(ev.get("timestamp")), "items": items})
    return calls


def first_ms(path):
    with open(path, "rb") as fh:
        for line in fh.read(200_000).splitlines()[:60]:
            m = re.search(rb'"timestamp":"([^"]+)"', line)
            if m:
                return swarm.iso_ms(m.group(1).decode())
    return None


# ---------------------------------------------------------------- steps

def declared_steps(calls, item_id):
    """Per stage: when it started, when it ended and the last note, from the work_set calls naming
    the item: the first call naming a stage starts it, the next stage change ends it."""
    st = {}
    cur = None
    for c in calls:
        it = next((i for i in c["items"] if item_id is None or str(i.get("id")) == item_id), None)
        if not it or not c["ms"]:
            continue
        stage = it.get("stage")
        if stage in KEYS:
            if stage != cur:
                if cur:
                    st[cur]["ended_ms"] = c["ms"]
                s = st.setdefault(stage, {"started_ms": c["ms"], "ended_ms": None, "note": None})
                s["ended_ms"] = None
                cur = stage
            if it.get("note"):
                st[stage]["note"] = str(it["note"])
        elif it.get("note") and cur:
            st[cur]["note"] = str(it["note"])
    return st, cur


# ---------------------------------------------------------------- attempts

def is_check(cmd):
    for seg in re.split(r"&&|;|\|\|", cmd):
        if CHECK.match(seg.strip()):
            return True
    return False


def kinds_of(cmd):
    k = set()
    for seg in re.split(r"&&|;|\|\|", cmd):
        s = seg.strip()
        if re.search(r"\b(?:run\s+)?check\b", s) and re.match(r"^(?:heavy\s+)?(?:bun|npm|pnpm|yarn)\b", s):
            k |= {"tests", "typecheck", "lint"}
        if re.search(r"typecheck|\btsc\b|\bmypy\b", s):
            k.add("typecheck")
        if re.search(r"\blint\b|eslint|biome|oxlint|clippy|ruff", s):
            k.add("lint")
        if re.search(r"\btest\b|vitest|jest|pytest|\bunittest\b|playwright|\be2e\b", s):
            k.add("tests")
    return k


def result_of(res, cmd_text):
    """(ok, output text, lines) of a Bash result: is_error, a non-zero `Exit code`, or a failure count
    in the text mark it failed."""
    lines = swarm.tool_output(res.get("content"))
    text = "\n".join(lines)
    failed = bool(res.get("is_error"))
    m = re.match(r"Exit code (\d+)", text)
    if m and m.group(1) != "0":
        failed = True
    echo = re.findall(r"^(?:exit(?: code)?|status|rc|EXIT)[ =:]+(\d+)\s*$", text, re.M | re.I)
    if echo and echo[-1] != "0":
        failed = True   # `cmd > log; echo exit $?` hides the exit code from is_error
    if not failed and re.search(r"(?<![\d.])[1-9]\d*\s+(?:failed|failing|fail)\b|\bFAIL\b\s|error TS\d+", text):
        failed = True
    return (not failed), text, lines


def parse_tests(text):
    """The last count the run printed: a run prints several (per file, per package), its summary last."""
    fails = [int(n) for n in re.findall(r"(?<![\d.])(\d+)\s+(?:failed|failing|fail)\b", text) if int(n) > 0]
    oks = re.findall(r"(?<![\d.])(\d+)\s+(?:passed|passing|pass)\b", text)
    if fails:
        return "%d fail" % fails[-1]
    if oks:
        return "%d pass" % int(oks[-1])
    m = re.search(r"(?<![\d./])(\d+)/(\d+)\s+(?:tests?|passed|specs?)\b", text)
    return "%s/%s" % m.groups() if m else None


def parse_typecheck(text):
    n = re.search(r"Found (\d+) errors?", text)
    k = int(n.group(1)) if n else len(re.findall(r"error TS\d+", text))
    return "%d error%s" % (k, "" if k == 1 else "s") if k else None


def parse_lint(text):
    n = re.search(r"(\d+)\s+problems?", text)
    if n:
        return "%d error%s" % (int(n.group(1)), "" if int(n.group(1)) == 1 else "s")
    k = len(re.findall(r"^\s*\d+:\d+\s+error\b", text, re.M))
    return "%d error%s" % (k, "" if k == 1 else "s") if k else None


def detail_of(ok, lines, kinds):
    """One line for the row: what the run said last (pass) or its first error line (fail)."""
    if not lines:
        return ""
    if ok:
        t = parse_tests("\n".join(lines)) if "tests" in kinds else None
        return t or ""
    for l in lines:
        if re.search(r"error|fail|✕|✖|FAIL|Cannot|not found|expected", l, re.I) and not l.startswith("Exit code"):
            return l.strip()[:160]
    return lines[0].strip()[:160]


def build_attempts(path, worktree, now_ms, idle):
    attempts, by_id = [], {}
    cluster = None
    last_edit_seq = {}   # command key -> (attempt, edit counter when it failed)
    edit_counter = 0

    def close_cluster():
        nonlocal cluster
        if cluster:
            files = cluster["files"]
            names = list(files)
            shown = ", ".join(os.path.basename(f) for f in names[:3]) + (", +%d" % (len(names) - 3) if len(names) > 3 else "")
            n = len(names)
            a = cluster["a"]
            a["what"] = "Edited %d file%s (%s)" % (n, "" if n == 1 else "s", shown)
            a["detail"] = "%d edit%s" % (cluster["n"], "" if cluster["n"] == 1 else "s")
            a["output"] = "\n".join("%s  ×%d" % (f, files[f]) for f in names)
            a["took_ms"] = max(0, (cluster["last"] or a["ms"]) - a["ms"])
            a["result"] = "ok" if cluster["ok"] else "fail"
            cluster = None

    for e, c, ms, _ in swarm.walk(swarm.read_tail(path, TAIL_BYTES)):
        if e.get("isSidechain"):
            continue
        if e.get("type") == "assistant" and c.get("type") == "tool_use":
            name, inp, uid = c.get("name", ""), c.get("input") or {}, c.get("id")
            if not isinstance(inp, dict) or name.endswith("work_set"):
                continue
            if name in EDITS:
                fp = inp.get("file_path") or inp.get("notebook_path") or ""
                fp = fp[len(worktree) + 1:] if worktree and fp.startswith(worktree + "/") else fp
                edit_counter += 1
                if not cluster:
                    a = {"id": sid(uid), "result": "running", "what": "", "ms": ms, "kind": "edit"}
                    attempts.append(a)
                    cluster = {"a": a, "files": {}, "n": 0, "last": ms, "ok": False}
                cluster["files"][fp] = cluster["files"].get(fp, 0) + 1
                cluster["n"] += 1
                cluster["last"] = ms
                cluster["uids"] = cluster.get("uids", set()) | {uid}
                by_id[uid] = ("edit", cluster)
            elif name == "Bash" and is_check(swarm.short_command(inp.get("command", ""), worktree)):
                close_cluster()
                cmd = swarm.short_command(inp.get("command", ""), worktree)
                a = {"id": sid(uid), "result": "running", "what": cmd[:300], "ms": ms, "kind": "check", "_cmd": cmd}
                attempts.append(a)
                by_id[uid] = ("check", a)
            elif name == "Agent":
                close_cluster()
                desc = str(inp.get("description") or inp.get("subagent_type") or "agent")
                a = {"id": sid(uid), "result": "running", "what": desc[:300], "ms": ms, "kind": "agent",
                     "detail": str(inp.get("subagent_type") or "")[:80]}
                attempts.append(a)
                by_id[uid] = ("agent", a)
        elif e.get("type") == "user" and c.get("type") == "tool_result":
            hit = by_id.get(c.get("tool_use_id"))
            if not hit:
                continue
            kind, obj = hit
            if kind == "edit":
                if not c.get("is_error"):
                    obj["ok"] = True
                continue
            a = obj
            ok, text, lines = result_of(c, a.get("_cmd", ""))
            a["took_ms"] = max(0, ms - a["ms"]) if ms and a["ms"] else None
            tail = lines[-OUT_LINES:]
            a["output"] = ("… %d earlier lines\n" % (len(lines) - OUT_LINES) if len(lines) > OUT_LINES else "") + "\n".join(tail)
            if kind == "agent":
                a["result"] = "fail" if c.get("is_error") else "ok"
                a["detail"] = (a.get("detail") or "") + (" · " if a.get("detail") else "") + (lines[0].strip()[:120] if lines else "")
                continue
            a["result"] = "ok" if ok else "fail"
            a["detail"] = detail_of(ok, lines, kinds_of(a["_cmd"]))
            a["_text"] = text
            key = a["_cmd"]
            prev = last_edit_seq.pop(key, None)
            if prev and edit_counter > prev[1]:
                prev[0]["result"] = "retry"   # failed, was edited, and the same command ran again
            if not ok:
                last_edit_seq[key] = (a, edit_counter)
    close_cluster()
    for a in attempts:
        if a["result"] == "running":
            if a["kind"] == "edit":
                a["result"] = "ok"
                continue
            if idle > 300_000:
                a["result"], a["detail"] = "fail", "no result recorded (interrupted)"
    return attempts


def checks_of(attempts, files, commits):
    c = {}
    for a in attempts:
        if a.get("kind") != "check" or a["result"] in ("running",):
            continue
        ok, text = a["result"] == "ok", a.get("_text", "")
        for k in kinds_of(a["_cmd"]):
            if k == "tests":
                v = parse_tests(text) or ("pass" if ok else "fail" if a["_cmd"] and not re.search(r"\bcheck\b", a["_cmd"]) else None)
            elif k == "typecheck":
                v = "pass" if ok else parse_typecheck(text) or (None if re.search(r"\bcheck\b", a["_cmd"]) else "fail")
            else:
                v = "pass" if ok else parse_lint(text) or (None if re.search(r"\bcheck\b", a["_cmd"]) else "fail")
            if v:
                c[k] = v
    add, dele = sum(f["add"] for f in files), sum(f["del"] for f in files)
    c["diff"] = "+%d −%d" % (add, dele)
    c["files"] = len(files)
    c["commits"] = len(commits)
    return c


# ---------------------------------------------------------------- item

def item_cache():
    try:
        with open(CACHE) as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return {}


def tidy(text):
    """A description for the page: markdown heading marks dropped and each line break shown as a dot, 1200 characters."""
    t = re.sub(r"^#+\s*", "", str(text or ""), flags=re.M)
    return re.sub(r"\s*\n+\s*", " \u00b7 ", t).strip()[:1200]


def lookup_item(cands, branch, repo_dir, dirname):
    """{id, title, desc?} for the first candidate (kind, id) bd or gh knows, else the branch name."""
    cache = item_cache()
    for kind, i in cands:
        key = "%s:%s:%s" % (repo_dir, kind, i)
        if key in cache:
            if cache[key]:
                return cache[key]
            continue
        found = None
        if kind in ("bead", "issue") and repo_dir:
            out = sh([swarm.tool("bd"), "show", i, "--json"], cwd=repo_dir, timeout=8)
            try:
                d = json.loads(out) if out else None
                d = d[0] if isinstance(d, list) and d else d
            except ValueError:
                d = None
            if isinstance(d, dict) and d.get("title"):
                found = {"id": i, "title": d["title"], "desc": tidy(d.get("description"))}
        if not found and kind in ("pr", "issue") and repo_dir:
            out = sh([swarm.tool("gh"), "pr" if kind == "pr" else "issue", "view", i, "--json", "title,body"], cwd=repo_dir, timeout=10)
            try:
                d = json.loads(out) if out else None
            except ValueError:
                d = None
            if isinstance(d, dict) and d.get("title"):
                found = {"id": ("PR #" if kind == "pr" else "#") + i, "title": d["title"], "desc": tidy(d.get("body"))}
        if found or repo_dir is not None:
            cache[key] = found or False
            try:
                with open(CACHE + ".tmp", "w") as fh:
                    json.dump(cache, fh)
                os.replace(CACHE + ".tmp", CACHE)
            except OSError:
                pass
        if found:
            return found
    return {"id": "", "title": branch or dirname}


def id_candidates(calls, decls, space_dir, branch):
    c = []
    for call in reversed(calls):
        for it in call["items"]:
            if it.get("id"):
                c.append((it.get("kind") or "bead", str(it["id"])))
    mine = [d for d in decls if d.get("cwd") == space_dir]
    for d in sorted(mine, key=lambda d: d["_ms"], reverse=True):
        for it in d.get("items") or []:
            if isinstance(it, dict) and it.get("id"):
                c.append((it.get("kind") or "bead", str(it["id"])))
    text = "%s %s" % (branch or "", os.path.basename(space_dir))
    for m in re.finditer(r"(?<![A-Za-z0-9])([A-Za-z][A-Za-z0-9]*(?:-[A-Za-z0-9]+)*-[a-z0-9]{3,5}(?:\.\d+)*)(?![A-Za-z0-9])", text):
        c.append(("bead", m.group(1)))
    out, seen = [], set()
    for x in c:
        if x not in seen:
            seen.add(x)
            out.append(x)
    return out[:4]


# ---------------------------------------------------------------- snapshot

def snapshot():
    t0 = time.time()
    now_ms = int(t0 * 1000)
    space_dir = os.environ.get("WHEEL_SPACE_DIR") or os.getcwd()
    g = git_state(space_dir)
    top = g["top"] if g else space_dir
    path = os.environ.get("IMPLEMENT_TRANSCRIPT") or newest_transcript(top)
    calls = work_set_calls(path) if path and os.path.isfile(path) else []
    # The latest pass in the session: the work_set calls naming the item the last call named.
    last_items = calls[-1]["items"] if calls else []
    item_id = str(last_items[0].get("id")) if last_items and last_items[0].get("id") else None
    mine = [c for c in calls if any(str(i.get("id")) == item_id for i in c["items"])] if item_id else calls
    declared, current = declared_steps(mine, item_id)
    on_default = bool(g and g["branch"] == g["default"])
    none = ({"state": "none", "now_ms": now_ms, "space_dir": space_dir}, "none", 0, t0)
    if not declared and (not g or on_default or not path):
        return none
    mtime_ms = int(os.path.getmtime(path) * 1000) if path and os.path.isfile(path) else None
    idle = now_ms - mtime_ms if mtime_ms else 0
    attempts = build_attempts(path, top, now_ms, idle) if path and os.path.isfile(path) else []
    g = g or {"branch": "", "commits": [], "files": [], "top": space_dir}
    if not declared and not attempts and not g["commits"]:
        return none

    # steps
    steps = []
    inferred = not declared
    if inferred:
        order = {"edit": "edit", "check": "verify", "agent": "review"}
        for a in attempts:
            a["_step"] = order[a["kind"]]
        reached = [k for k in KEYS if any(a["_step"] == k for a in attempts)]
        current = reached[-1] if reached else None
        for k in reached:
            ms = [a["ms"] for a in attempts if a["_step"] == k and a["ms"]]
            declared[k] = {"started_ms": min(ms), "ended_ms": None, "note": "inferred from tool calls"}
        for k in reached[:-1]:
            declared[k]["ended_ms"] = declared[reached[reached.index(k) + 1]]["started_ms"]
    else:
        spans = sorted(((s["started_ms"], k) for k, s in declared.items()))
        for a in attempts:
            at = next((k for t, k in reversed(spans) if a["ms"] and a["ms"] >= t), spans[0][1])
            a["_step"] = at
    for k, label in STEPS:
        s = declared.get(k)
        row = {"key": k, "label": label, "state": "todo", "attempts": []}
        if s:
            row.update(state="now" if k == current else "done", started_ms=s["started_ms"])
            if s["ended_ms"] and k != current:
                row["ended_ms"] = s["ended_ms"]
            elif k != current:
                row["ended_ms"] = s["started_ms"]
            if s.get("note"):
                row["summary"] = s["note"]
        steps.append(row)
    attempts = attempts[-MAX_ATTEMPTS:]
    for n, a in enumerate(reversed(attempts)):
        if "output" in a:
            a["output"] = a["output"][-(OUT_CHARS if n < FULL_OUT else OLD_OUT_CHARS):]
    for a in attempts:
        st = next(s for s in steps if s["key"] == a["_step"])
        row = {k: v for k, v in a.items() if not k.startswith("_") and k != "kind" and v is not None}
        st["attempts"].append(row)
    chk = checks_of(attempts, g["files"], g["commits"])

    decls = swarm.declarations()
    repo = swarm.main_checkout(top) if g.get("branch") else None
    item = lookup_item(id_candidates(mine, decls, top, g["branch"]), g["branch"], repo or None, os.path.basename(top))
    started = min([s["started_ms"] for s in declared.values()] + [c["ms"] for c in g["commits"]] or [now_ms])
    if not declared:
        started = first_ms(path) or now_ms
    snap = {"state": "ok", "item": item, "branch": g["branch"], "started_ms": started, "now_ms": now_ms,
            "stage": current or "", "steps": steps, "checks": chk,
            "files": g["files"], "commits": g["commits"], "transcript": path}
    return snap, g["branch"], len(attempts), t0


def main():
    try:
        data, branch, n, t0 = snapshot()
    except Exception as e:  # the page shows the failure instead of the last good snapshot
        import traceback
        traceback.print_exc()
        data, branch, n, t0 = {"state": "error", "error": "%s: %s" % (type(e).__name__, e), "now_ms": int(time.time() * 1000)}, "error", 0, time.time()
    body = json.dumps(data)
    if os.environ.get("WHEEL_PANE"):
        r = subprocess.run(["wheel", "artifact", "data", os.environ["WHEEL_PANE"], "-"], input=body,
                           capture_output=True, text=True, timeout=20)
        if r.returncode != 0:
            sys.stderr.write(r.stderr)
            sys.exit(r.returncode)
    else:
        print(body)
    sys.stderr.write("implement: %s: %d attempts, %d ms, %d bytes\n" % (branch, n, (time.time() - t0) * 1000, len(body)))
    if data.get("state") == "error":
        sys.exit(1)


if __name__ == "__main__":
    main()
