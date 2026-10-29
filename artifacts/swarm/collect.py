"""Refresh command for the swarm artifact: pushes one JSON snapshot of a single swarm run to the
pane with `wheel artifact data "$WHEEL_PANE" -`, so the page redraws without reloading.

Which run: the repo the page picked (`wheel.set("swarm", "<repo dir>")`), else the repo of the
space the pane is in ($WHEEL_SPACE_DIR, a worktree resolved to its main checkout). Never more
than one run.

Sources, all read-only:
  `swarm status`                         the run's log lines, the beads in flight with their agent
                                         state and pane, the sign-off inbox
  ~/.local/state/work-board/sessions/    the lead's declaration (goal, lanes) and each worker's
                                         stage and note; a worker's file names its Claude session
  <config dir>/projects/<cwd>/<id>.jsonl each worker's transcript: its latest actions, and its
                                         last write time, which the page compares against its
                                         stuck threshold
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
ACTIONS = 14          # latest actions kept per worker
TAIL_BYTES = 400_000  # how much of a transcript's end is read for them
OTHERS_EVERY = 60     # seconds the other spaces' list is reused before it is read again
# Bead titles and the other spaces' runs, kept between runs for this pane; never in the data
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


def picked(pane):
    """The repo the page asked for, or None."""
    if not pane:
        return None
    code, out, _ = sh(["wheel", "artifact", "state", pane, "swarm"], timeout=10)
    if code != 0:
        return None
    try:
        v = json.loads(out)
    except ValueError:
        return None
    return v if isinstance(v, str) and v else None


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


def transcript(worktree, session_id, since_ms):
    """The worker's transcript path: its declared session's file, else the newest one Claude wrote
    for the worktree since the launch."""
    enc = re.sub(r"[^A-Za-z0-9]", "-", worktree)
    best = None
    for c in config_dirs():
        d = os.path.join(c, "projects", enc)
        if session_id and os.path.isfile(os.path.join(d, session_id + ".jsonl")):
            return os.path.join(d, session_id + ".jsonl")
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
        return inp.get("description") or inp.get("command", "")
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


def actions_of(path, worktree):
    """The transcript's latest actions, oldest first: tool calls (with whether each returned),
    and what the agent said."""
    size = os.path.getsize(path)
    with open(path, "rb") as fh:
        fh.seek(max(0, size - TAIL_BYTES))
        raw = fh.read().decode("utf-8", "replace")
    lines = raw.splitlines()
    if size > TAIL_BYTES:
        lines = lines[1:]  # the first is cut mid-line
    acts, by_id = [], {}
    for line in lines:
        try:
            e = json.loads(line)
        except ValueError:
            continue
        if not isinstance(e, dict) or not isinstance(e.get("message"), dict):
            continue
        content = e["message"].get("content")
        if not isinstance(content, list):
            continue
        ms = iso_ms(e.get("timestamp"))
        for c in content:
            if not isinstance(c, dict):
                continue
            if e.get("type") == "assistant" and c.get("type") == "tool_use":
                a = {"ms": ms, "kind": "tool", "tool": c.get("name", ""),
                     "text": str(summarize_tool(c.get("name", ""), c.get("input"), worktree))[:240], "done": None}
                acts.append(a)
                by_id[c.get("id")] = a
            elif e.get("type") == "assistant" and c.get("type") == "text" and str(c.get("text") or "").strip():
                acts.append({"ms": ms, "kind": "said", "tool": "", "text": str(c["text"]).strip()[:400]})
            elif e.get("type") == "user" and c.get("type") == "tool_result":
                a = by_id.get(c.get("tool_use_id"))
                if a:
                    a["done"] = ms
                    a["error"] = bool(c.get("is_error"))
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


def others(current, cache):
    """The swarm runs of every space's repo, for the page's picker: one row per repo."""
    hit = cache.get("others")
    if hit and time.time() - hit["at"] < OTHERS_EVERY:
        return [dict(r, current=r["repo"] == current) for r in hit["rows"]]
    code, out, _ = sh(["wheel", "space", "ls", "--json"], timeout=10)
    try:
        spaces = json.loads(out) if code == 0 else []
    except ValueError:
        spaces = []
    seen, rows = set(), []
    for s in spaces:
        d = s.get("dir") or s.get("cwd")
        repo = main_checkout(d) if d and os.path.isdir(d) else None
        if not repo or repo in seen:
            continue
        seen.add(repo)
        log = run_log_path(repo)
        if log and os.path.isfile(log):
            rows.append({"repo": repo, "name": os.path.basename(repo), "current": repo == current,
                         "log_ms": int(os.path.getmtime(log) * 1000)})
    rows.sort(key=lambda r: -r["log_ms"])
    cache["others"] = {"at": time.time(), "rows": rows}
    return rows


def snapshot(cache):
    pane = os.environ.get("WHEEL_PANE", "")
    space_dir = os.environ.get("WHEEL_SPACE_DIR") or os.getcwd()
    choice = picked(pane)
    repo = main_checkout(choice or space_dir)
    data = {"now_ms": int(time.time() * 1000), "space_dir": space_dir, "picked": choice,
            "repo": repo, "name": os.path.basename(repo) if repo else None, "others": others(repo, cache)}
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
    pid = re.search(r"pid=(\d+)", run["detail"]).group(1) if run and re.search(r"pid=(\d+)", run["detail"]) else None
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
            workers[b] = {"bead": b, "events": []}
            order.append(b)
        w = workers[b]
        w["events"].append({"ms": r["ms"], "event": r["event"], "detail": r["detail"]})
        if r["event"] == "launched":
            parts = r["detail"].split(" ", 1)
            w.update(model=parts[0], worktree=parts[1] if len(parts) > 1 else "", launched_ms=r["ms"],
                     outcome=None, outcome_detail="", outcome_ms=None)
        elif r["event"] in ("landed", "parked", "failed", "finished", "ready", "signed", "declined"):
            if r["event"] != "finished" or not w.get("outcome"):
                w.update(outcome=r["event"], outcome_detail=r["detail"], outcome_ms=r["ms"])
    titles = bead_titles(repo, order, cache)
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
            details(w, b, decls)
        except Exception as e:
            w["details_error"] = "%s: %s" % (type(e).__name__, e)
        rows.append(w)
    data["workers"] = rows
    return data


def details(w, b, decls):
    """A worker's declared stage, its transcript and its latest actions."""
    wt = w.get("worktree", "")
    # The worker's own session is the one in its worktree that declared its bead; another session
    # there (a later review pass) declares something else.
    mine = [d for d in decls if d.get("cwd") == wt and d["_ms"] >= (w["launched_ms"] or 0) - 60_000
            and any(isinstance(i, dict) and i.get("id") == b for i in d.get("items") or [])]
    decl = max(mine, key=lambda d: d["_ms"]) if mine else None
    if decl:
        item = next(i for i in decl["items"] if isinstance(i, dict) and i.get("id") == b)
        w["stage"], w["note"], w["stage_ms"] = item.get("stage"), item.get("note"), decl["_ms"]
    path = transcript(wt, decl.get("sessionId") if decl else None, w["launched_ms"]) if wt else None
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
