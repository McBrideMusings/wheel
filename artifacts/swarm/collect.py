"""Refresh command for the swarm artifact: pushes one JSON snapshot of a single swarm run to the
pane with `wheel artifact data "$WHEEL_PANE" -`, so the page redraws without reloading.

Which run: the one of the repo of the space the pane is in ($WHEEL_SPACE_DIR, a worktree
resolved to its main checkout). Never another space's, never more than one.

Sources, all read-only:
  `swarm status`                         the run's log lines, the beads in flight with their agent
                                         state and pane, the sign-off inbox
  ~/.local/state/work-board/sessions/    the lead's declaration (goal, lanes) and each worker's
                                         stage and note; a worker's file names its Claude session
  <config dir>/projects/<cwd>/<id>.jsonl each worker's transcript: its latest actions (tool calls,
                                         what it said, and each run of thinking with its time),
                                         and its last write time, which the page compares
                                         against its 10-minute stuck threshold

Each bead in the run is one row with every attempt at it; `state` is where it stands now: the
verdict of its latest attempt (`landed`, `reopened`, `parked`, `failed`, or `stopped` when the
lead logged none). A `failed` whose detail starts "not verified" is a reopen logged before the
`reopened` event existed.
"""
import calendar
import glob
import json
import os
import re
import shutil
import subprocess
import sys
import time

HOME = os.path.expanduser("~")
SWARM = os.path.join(HOME, ".claude/skills/swarm/swarm")
SESSIONS = os.path.join(HOME, ".local/state/work-board/sessions")
ACTIONS = 24          # latest actions kept per worker
OUT_LINES = 4         # lines of a tool's output kept for the chat view
TAIL_BYTES = 400_000  # how much of a transcript's end is read for them
# Bead titles, kept between runs for this pane; never in the data
# directory, where every write reloads the page.
CACHE = os.path.join(os.environ.get("TMPDIR", "/tmp"), "wheel-swarm-%s.json" % os.environ.get("WHEEL_PANE", "cli"))


def sh(args, cwd=None, env=None, timeout=30):
    """(exit, stdout, stderr); a command past its timeout is exit 124, so one slow source costs
    only its own part of the snapshot."""
    try:
        r = subprocess.run(args, cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return 124, "", "%s timed out after %ss" % (os.path.basename(args[0]), timeout)
    return r.returncode, r.stdout, r.stderr


def tool(name):
    return shutil.which(name) or os.path.join("/opt/homebrew/bin", name)


def iso_ms(t):
    """'2026-10-29T18:01:41Z' or '…:41.123Z' → unix ms, or None."""
    if not t:
        return None
    m = re.match(r"(\d{4})-(\d\d)-(\d\d)T(\d\d):(\d\d):(\d\d)(?:\.(\d+))?Z", t)
    if not m:
        return None
    secs = calendar.timegm(tuple(int(x) for x in m.groups()[:6]))
    return secs * 1000 + int((m.group(7) or "0")[:3].ljust(3, "0"))


def main_checkout(d):
    """The main checkout of the repo holding d, or None when d is in no repo."""
    code, out, _ = sh(["git", "-C", d, "rev-parse", "--path-format=absolute", "--git-common-dir"])
    if code != 0:
        return None
    common = out.strip()
    if os.path.basename(common) == ".git":
        return os.path.dirname(common)
    code, out, _ = sh(["git", "-C", d, "rev-parse", "--show-toplevel"])
    return out.strip() if code == 0 else None


def run_log_path(repo):
    code, out, _ = sh([os.path.join(HOME, ".claude/tools/repo-slug"), "--path"], cwd=repo, timeout=10)
    return os.path.join(out.strip(), "swarm", "run.tsv") if code == 0 and out.strip() else None


def swarm_status(repo):
    """(log rows, {bead: (agent, pane)}, inbox rows) from `swarm status`, or raises."""
    env = {k: v for k, v in os.environ.items() if k not in ("WHEEL_SPACE", "WHEEL_PANE")}
    env["WHEEL_ENV"] = "1"
    code, out, err = sh([SWARM, "status"], cwd=repo, env=env, timeout=30)
    if code != 0:
        raise RuntimeError((err.strip().splitlines() or ["swarm status exit %d" % code])[-1])
    log, flight, inbox, part = [], {}, [], "log"
    for line in out.splitlines():
        if line.startswith("--- in flight"):
            part = "flight"; continue
        if line.startswith("--- sign-off inbox"):
            part = "inbox"; continue
        cols = line.split("\t")
        if part == "log" and len(cols) >= 2:
            cols += [""] * (4 - len(cols))
            log.append({"t": cols[0], "ms": iso_ms(cols[0]), "event": cols[1], "bead": cols[2], "detail": cols[3]})
        elif part == "flight" and len(cols) >= 3:
            m = re.match(r"agent (.+?)(?: \(pane (\w+)\))?$", cols[2])
            flight[cols[0]] = (m.group(1), m.group(2) or "") if m else (cols[2], "")
        elif part == "inbox" and cols[0]:
            inbox.append({"bead": cols[0], "t": cols[1] if len(cols) > 1 else "", "detail": cols[2] if len(cols) > 2 else ""})
    return log, flight, inbox


def declarations():
    out = []
    for f in glob.glob(os.path.join(SESSIONS, "*.json")):
        try:
            with open(f) as fh:
                d = json.load(fh)
        except (OSError, ValueError):
            continue
        if not isinstance(d, dict):
            continue
        d["_ms"] = iso_ms(d.get("updatedAt")) or 0
        out.append(d)
    return out


def config_dirs():
    dirs = [os.path.join(HOME, ".claude")] + sorted(glob.glob(os.path.join(HOME, ".claude-*")))
    return [d for d in dirs if os.path.isdir(os.path.join(d, "projects"))]


def launched_session(repo, bead):
    """The conversation id `wheel-agent` gave the worker on its command line (`--session-id`),
    kept in the worktree's git dir; None when the worktree is gone or ran under herdr."""
    try:
        with open(os.path.join(repo, ".git", "worktrees", bead, "wheel-agent", "session")) as f:
            return f.read().strip() or None
    except OSError:
        return None


def transcript(worktree, session_id, since_ms):
    """The worker's transcript path: its session's file in whichever project folder Claude wrote
    it to, else the newest one Claude wrote for the worktree since the launch."""
    enc = re.sub(r"[^A-Za-z0-9]", "-", worktree)
    if session_id:
        for c in config_dirs():
            hit = glob.glob(os.path.join(c, "projects", "*", session_id + ".jsonl"))
            if hit:
                return hit[0]
    best = None
    for c in config_dirs():
        d = os.path.join(c, "projects", enc)
        for f in glob.glob(os.path.join(d, "*.jsonl")):
            try:
                m = os.path.getmtime(f)
            except OSError:
                continue
            if since_ms and m * 1000 < since_ms - 60_000:
                continue
            if best is None or m > best[0]:
                best = (m, f)
    return best[1] if best else None


def summarize_tool(name, inp, worktree):
    def rel(p):
        p = str(p or "")
        return p[len(worktree) + 1:] if worktree and p.startswith(worktree + "/") else p
    if not isinstance(inp, dict):
        return ""
    if name == "Bash":
        return short_command(inp.get("command", ""), worktree)
    if name in ("Read", "Edit", "Write", "NotebookEdit"):
        return rel(inp.get("file_path") or inp.get("notebook_path"))
    if name in ("Grep", "Glob"):
        return inp.get("pattern", "")
    if name == "Skill":
        return " ".join(x for x in (inp.get("skill"), inp.get("args")) if x)
    if name == "Agent":
        return inp.get("description", "")
    if name.endswith("work_set"):
        items = inp.get("items") or []
        return "; ".join(" ".join(str(x) for x in (i.get("stage"), i.get("note")) if x)
                         for i in items if isinstance(i, dict))
    for v in inp.values():
        if isinstance(v, str) and v:
            return v
    return ""


def short_command(cmd, worktree):
    """A command's first line with the noise in front of it gone: a `( cd <dir> &&` or `cd <dir> &&`,
    `env -u` and VAR=value assignments, the swarm's `heavy --` wrapper (kept as `heavy`), and the
    worktree's own path, which reads as `.`."""
    t = str(cmd or "").split("\n")[0].strip()
    if re.match(r"^\(\s*cd\s", t):
        t = re.sub(r"\s*\)\s*$", "", t)
    t = re.sub(r"^\(?\s*cd\s+[^&;]+(?:&&|;)\s*", "", t)
    t = re.sub(r'^"?(?:\$HOME|~|%s)/\.claude/skills/swarm/swarm"?\s+heavy\s+--\s*' % re.escape(HOME), "heavy ", t)
    m = re.match(r"^(heavy\s+)?bash\s+-c\s+(['\"])(.*?)\2?$", t)
    if m:
        t = (m.group(1) or "") + m.group(3)
    t = re.sub(r"^(heavy\s+)?env(?:\s+-u\s+\S+)*\s+", lambda m: m.group(1) or "", t)
    t = re.sub(r"^(heavy\s+)?(?:\w+=\S+\s+)+", lambda m: m.group(1) or "", t)
    if worktree:
        t = re.sub(re.escape(worktree) + r"(?=[/\s'\")]|$)", ".", t)
    return t


def tool_output(content):
    """A tool result's text as non-empty lines: the result is a string or a list of text blocks."""
    if isinstance(content, list):
        content = "\n".join(str(b.get("text", "")) for b in content if isinstance(b, dict))
    return [l.rstrip() for l in str(content or "").splitlines() if l.strip()]


def read_tail(path, tail_bytes):
    """A transcript's last `tail_bytes` as whole lines (the first, cut mid-line, is dropped)."""
    size = os.path.getsize(path)
    with open(path, "rb") as fh:
        fh.seek(max(0, size - tail_bytes))
        raw = fh.read().decode("utf-8", "replace")
    lines = raw.splitlines()
    return lines[1:] if size > tail_bytes else lines


def walk(lines):
    """Every content block of a transcript's JSONL lines, in order, as (event, block, ms, prev):
    ms is the event's time and prev the time of the last event before it that had one."""
    prev = None
    for line in lines:
        try:
            e = json.loads(line)
        except ValueError:
            continue
        if not isinstance(e, dict):
            continue
        ms = iso_ms(e.get("timestamp"))
        content = e["message"].get("content") if isinstance(e.get("message"), dict) else None
        if isinstance(content, list):
            for c in content:
                if isinstance(c, dict):
                    yield e, c, ms, prev
        prev = ms or prev


def actions_of(path, worktree):
    """The transcript's latest actions, oldest first: tool calls (with whether each returned),
    what the agent said (whole), the first lines each tool printed, and each run of thinking blocks as one `think` action, timed from the
    event before the run to its last block. A thinking block's text is not stored, so only its
    time can show."""
    acts, by_id = [], {}
    for e, c, ms, prev in walk(read_tail(path, TAIL_BYTES)):
        if e.get("type") == "assistant" and c.get("type") == "thinking":
            if acts and acts[-1]["kind"] == "think":
                acts[-1]["done"] = ms
            else:
                acts.append({"ms": prev or ms, "kind": "think", "tool": "", "text": "thinking", "done": ms})
        elif e.get("type") == "assistant" and c.get("type") == "tool_use":
            a = {"ms": ms, "kind": "tool", "tool": c.get("name", ""),
                 "text": str(summarize_tool(c.get("name", ""), c.get("input"), worktree))[:240], "done": None}
            acts.append(a)
            by_id[c.get("id")] = a
        elif e.get("type") == "assistant" and c.get("type") == "text" and str(c.get("text") or "").strip():
            acts.append({"ms": ms, "kind": "said", "tool": "", "text": str(c["text"]).strip()[:1500]})
        elif e.get("type") == "user" and c.get("type") == "tool_result":
            a = by_id.get(c.get("tool_use_id"))
            if a:
                a["done"] = ms
                a["error"] = bool(c.get("is_error"))
                out = tool_output(c.get("content"))
                a["out"] = [l[:200] for l in out[:OUT_LINES]]
                a["more"] = max(0, len(out) - OUT_LINES)
    return acts[-ACTIONS:]


def load_cache():
    try:
        with open(CACHE) as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return {}


def save_cache(cache):
    tmp = CACHE + ".tmp"
    with open(tmp, "w") as fh:
        json.dump(cache, fh)
    os.replace(tmp, CACHE)


def bead_titles(repo, ids, cache):
    """Titles from bd, which takes tens of seconds while a swarm uses it (one `bd list` of every
    bead costs about what `bd show` costs for three), so the repo's titles are read in one call
    and read again only when a bead in the run is missing from them."""
    known = cache.setdefault("titles", {}).setdefault(repo, {})
    if any(i not in known for i in ids):
        code, out, _ = sh([tool("bd"), "list", "--all", "--json", "--limit", "0"], cwd=repo, timeout=20)
        try:
            rows = json.loads(out) if code == 0 else []
        except ValueError:
            rows = []
        rows = rows if isinstance(rows, list) else []
        known.update({r["id"]: r.get("title", "") for r in rows if isinstance(r, dict) and r.get("id")})
        if rows:
            known.update({i: "" for i in ids if i not in known})  # not in this repo's tracker
    return known


def alive(pid):
    try:
        os.kill(int(pid), 0)
        return True
    except (ValueError, ProcessLookupError):
        return False
    except PermissionError:
        return True


def snapshot(cache):
    space_dir = os.environ.get("WHEEL_SPACE_DIR") or os.getcwd()
    repo = main_checkout(space_dir)
    data = {"now_ms": int(time.time() * 1000), "space_dir": space_dir,
            "repo": repo, "name": os.path.basename(repo) if repo else None}
    if not repo:
        data["state"] = "no-repo"
        return data
    log_path = run_log_path(repo)
    data["log_path"] = log_path
    if not log_path or not os.path.isfile(log_path):
        data["state"] = "no-run"
        return data

    log, flight, inbox = swarm_status(repo)
    data["inbox"] = inbox
    data["log"] = log
    run = next((r for r in log if r["event"] == "run"), None)
    start_ms = run["ms"] if run else None
    # The lead's pid: the newest of the `run` line's and the `lead` lines a lead that took the
    # run over writes.
    pid = None
    for r in log:
        m = re.search(r"pid=(\d+)", r["detail"]) if r["event"] in ("run", "lead") else None
        pid = m.group(1) if m else pid
    ended = next((r for r in log if r["event"] == "end"), None)
    data["run"] = {"started_ms": start_ms, "focus": run["detail"] if run else "",
                   "lead_pid": pid, "lead_alive": alive(pid) if pid else None,
                   "ended_ms": ended["ms"] if ended else None, "end_note": ended["detail"] if ended else ""}
    data["state"] = "ended" if ended else "live"

    decls = declarations()
    leads = [d for d in decls if d.get("cwd") == repo and (d.get("goal") or d.get("lanes"))
             and (not start_ms or d["_ms"] >= start_ms)]
    lead = max(leads, key=lambda d: d["_ms"]) if leads else None
    if lead:
        data["lead"] = {"session": lead.get("sessionId"), "goal": lead["goal"].get("id") if isinstance(lead.get("goal"), dict) else lead.get("goal"),
                        "lanes": lead.get("lanes"), "updated_ms": lead["_ms"],
                        "items": [{k: i.get(k) for k in ("id", "lane", "model", "stage", "note")}
                                  for i in lead.get("items") or [] if isinstance(i, dict)]}

    workers, order = {}, []
    for r in log:
        b = r["bead"]
        if not b:
            continue
        if b not in workers:
            workers[b] = {"bead": b, "attempts": []}
            order.append(b)
        w = workers[b]
        if r["event"] == "launched":
            parts = r["detail"].split(" ", 1)
            w["attempts"].append({"model": parts[0], "launched_ms": r["ms"], "events": []})
            w.update(model=parts[0], worktree=parts[1] if len(parts) > 1 else "", launched_ms=r["ms"])
        elif w["attempts"]:
            w["attempts"][-1]["events"].append({"ms": r["ms"], "event": r["event"], "detail": r["detail"]})
    for w in workers.values():
        for a in w["attempts"]:
            a.update(verdict(a["events"]))
        last = w["attempts"][-1] if w["attempts"] else {}
        w.update(state=last.get("state"), state_detail=last.get("detail", ""), state_ms=last.get("ms"))
    queued = [i["id"] for i in (data.get("lead") or {}).get("items", []) if i.get("id") and i["id"] not in workers]
    titles = bead_titles(repo, order + queued, cache)
    data["titles"] = {i: titles.get(i, "") for i in queued}
    rows = []
    for b in order:
        w = workers[b]
        if "launched_ms" not in w:
            continue  # filed or noted, never launched in this run
        w["title"] = titles.get(b, "")
        if b in flight:
            w["agent"], w["pane"] = flight[b]
            w["in_flight"] = True
        else:
            w["in_flight"] = False
        try:
            details(w, b, decls, repo)
        except Exception as e:
            w["details_error"] = "%s: %s" % (type(e).__name__, e)
        rows.append(w)
    data["workers"] = rows
    return data


VERDICTS = ("landed", "reopened", "parked", "failed", "ready", "signed", "declined")


def verdict(events):
    """Where one attempt stands: its last verdict the lead logged, or `stopped` when it finished
    with none. An old reopen was logged `failed` with a "not verified" detail."""
    v = next((e for e in reversed(events) if e["event"] in VERDICTS), None)
    if not v:
        f = next((e for e in reversed(events) if e["event"] == "finished"), None)
        return {"state": "stopped" if f else None, "detail": f["detail"] if f else "", "ms": f["ms"] if f else None}
    state = "reopened" if v["event"] == "failed" and v["detail"].startswith("not verified") else v["event"]
    return {"state": state, "detail": v["detail"], "ms": v["ms"]}


def details(w, b, decls, repo):
    """A worker's declared stage, its transcript and its latest actions."""
    wt = w.get("worktree", "")
    sid = launched_session(repo, b)
    # The worker's own session is the one launched with its conversation id, else the one in its
    # worktree that declared its bead; another session there (a later review pass) declares
    # something else.
    mine = [d for d in decls if (d.get("sessionId") == sid if sid else d.get("cwd") == wt)
            and d["_ms"] >= (w["launched_ms"] or 0) - 60_000
            and any(isinstance(i, dict) and i.get("id") == b for i in d.get("items") or [])]
    decl = max(mine, key=lambda d: d["_ms"]) if mine else None
    if decl:
        item = next(i for i in decl["items"] if isinstance(i, dict) and i.get("id") == b)
        w["stage"], w["note"], w["stage_ms"] = item.get("stage"), item.get("note"), decl["_ms"]
    path = transcript(wt, sid or (decl.get("sessionId") if decl else None), w["launched_ms"]) if wt else None
    if path:
        w["transcript"] = path
        w["last_write_ms"] = int(os.path.getmtime(path) * 1000)
        if w["in_flight"]:
            w["actions"] = actions_of(path, wt)


def main():
    cache = load_cache()
    try:
        data = snapshot(cache)
        save_cache(cache)
    except Exception as e:  # the page shows the failure instead of the last good snapshot
        data = {"now_ms": int(time.time() * 1000), "state": "error", "error": "%s: %s" % (type(e).__name__, e)}
    body = json.dumps(data)
    if os.environ.get("WHEEL_PANE"):
        r = subprocess.run(["wheel", "artifact", "data", os.environ["WHEEL_PANE"], "-"], input=body,
                           capture_output=True, text=True, timeout=20)
        if r.returncode != 0:
            sys.stderr.write(r.stderr)
            sys.exit(r.returncode)
    else:
        print(body)
    if data.get("state") == "error":
        sys.stderr.write(data["error"] + "\n")
        sys.exit(1)


if __name__ == "__main__":
    main()
