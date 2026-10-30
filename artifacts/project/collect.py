"""Refresh command for the project artifact: pushes one JSON snapshot of the beads repo of the space
the pane is in to the pane with `wheel artifact data "$WHEEL_PANE" -`, so the page redraws without
reloading.

Which repo: the one of the space's directory ($WHEEL_SPACE_DIR, a worktree resolved to its main
checkout; a folder outside git is taken as it is). Never another space's (ADR-0019).

Sources, all read-only, run in that directory:
  bd list --all --brief --json --limit 0   every bead without its long fields
  bd list --json --limit 0                 the beads not closed, with them
  bd ready --json --limit 0                which open beads bd counts as ready
  docs/CONTEXT.md                          the vision: its first paragraph after the title

A closed bead travels without its long fields (description, notes, design, acceptance); the page
asks for those, and for any bead's comments, when the bead is clicked:

  collect.py --bead <id>   prints `bd show <id> --json --include-comments --brief-deps` as one
                           JSON line (the page runs it with wheel.run and reads the run's log),
                           and keeps the newest 20 run logs

In that record a local image the bead's text names (`![](path)` or `<img src="path">`, absolute,
`~/` or relative to the repo) is inlined as a data: URL, since the pane serves only the artifact's
folder and its data directory; the snapshot keeps the path. Each run writes one line to stderr
naming the repo, the bead count and what bd took.
"""
import base64
import hashlib
import json
import mimetypes
import os
import re
import shutil
import subprocess
import sys
import time

LONG = ("description", "notes", "design", "acceptance_criteria")
IMAGE_MAX = 5 * 1024 * 1024  # a larger local image stays a path
IMAGE_TYPES = {".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg"}
RUNS_KEPT = 20  # run logs of bead clicks kept in the pane's runs/ folder


def sh(args, cwd=None, timeout=60):
    """(exit, stdout, stderr, seconds); a command past its timeout is exit 124."""
    t = time.time()
    try:
        r = subprocess.run(args, cwd=cwd, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return 124, "", "%s timed out after %ss" % (os.path.basename(args[0]), timeout), time.time() - t
    except OSError as e:
        return 127, "", str(e), time.time() - t
    return r.returncode, r.stdout, r.stderr, time.time() - t


def bd():
    return shutil.which("bd") or "/opt/homebrew/bin/bd"


def first_line(err):
    """A command's error: the first line of its stderr, where bd puts the `Error:` line."""
    return next((l.strip() for l in err.splitlines() if l.strip()), "no output")


def log(msg):
    sys.stderr.write("project: %s\n" % msg)
    sys.stderr.flush()


def main_checkout(d):
    """The main checkout of the repo holding d, or d itself when it is in no repo."""
    code, out, _, _ = sh(["git", "-C", d, "rev-parse", "--path-format=absolute", "--git-common-dir"], timeout=10)
    if code != 0:
        return d
    common = out.strip()
    if os.path.basename(common) == ".git":
        return os.path.dirname(common)
    code, out, _, _ = sh(["git", "-C", d, "rev-parse", "--show-toplevel"], timeout=10)
    return out.strip() if code == 0 and out.strip() else d


def vision(repo):
    """The first paragraph of docs/CONTEXT.md after its title, or None."""
    try:
        with open(os.path.join(repo, "docs", "CONTEXT.md")) as fh:
            text = fh.read()
    except OSError:
        return None
    para = []
    for line in text.splitlines():
        s = line.strip()
        if not s:
            if para:
                break
            continue
        if s.startswith("#") or s.startswith("---"):
            if para:
                break
            continue
        para.append(s)
    return " ".join(para) or None


def inline_images(text, repo):
    """text with each local image it names replaced by a data: URL."""
    if not text or ("![" not in text and "<img" not in text):
        return text

    def data_url(src):
        if re.match(r"^(https?:|data:)", src):
            return src
        p = os.path.expanduser(src)
        p = p if os.path.isabs(p) else os.path.join(repo, p)
        ext = os.path.splitext(p)[1].lower()
        try:
            if ext not in IMAGE_TYPES or os.path.getsize(p) > IMAGE_MAX:
                return src
            with open(p, "rb") as fh:
                raw = fh.read()
        except OSError:
            return src
        mime = mimetypes.guess_type(p)[0] or "image/png"
        return "data:%s;base64,%s" % (mime, base64.b64encode(raw).decode())

    text = re.sub(r"(!\[[^\]]*\]\()([^)\s]+)", lambda m: m.group(1) + data_url(m.group(2)), text)
    return re.sub(r"(<img\b[^>]*\bsrc=[\"'])([^\"']+)", lambda m: m.group(1) + data_url(m.group(2)), text)


def slim(r):
    """One bead as the page reads it; a closed bead without its long fields."""
    b = {
        "id": r.get("id"), "title": r.get("title", ""), "status": r.get("status"),
        "type": r.get("issue_type"), "priority": r.get("priority"), "parent": r.get("parent"),
        "labels": r.get("labels") or [], "assignee": r.get("assignee"),
        "created": r.get("created_at"), "updated": r.get("updated_at"), "closed": r.get("closed_at"),
        "close_reason": r.get("close_reason"), "comments": r.get("comment_count", 0),
        "deps": [{"id": d.get("depends_on_id"), "type": d.get("type")}
                 for d in r.get("dependencies") or [] if d.get("type") != "parent-child"],
    }
    for k in LONG:
        if r.get(k):
            b[k] = r[k]
    return b


def snapshot(data):
    """Fill data, which names the repo, with its beads."""
    repo = data["repo"]
    if not os.path.isdir(os.path.join(repo, ".beads")):
        data["state"] = "no-beads"
        log("%s has no .beads directory" % repo)
        return data
    # Every bead without its long fields, then the beads not closed with them.
    code, out, err, t_list = sh([bd(), "list", "--all", "--brief", "--json", "--limit", "0"], cwd=repo)
    if code != 0:
        raise RuntimeError("bd list exit %d: %s" % (code, first_line(err)))
    rows = json.loads(out or "[]")
    code, out, err, t_open = sh([bd(), "list", "--json", "--limit", "0"], cwd=repo)
    if code != 0:
        raise RuntimeError("bd list exit %d: %s" % (code, first_line(err)))
    long = {r["id"]: r for r in json.loads(out or "[]") if isinstance(r, dict) and r.get("id")}
    rows = [dict(r, **{k: long[r["id"]][k] for k in LONG if long.get(r.get("id"), {}).get(k)})
            for r in rows if isinstance(r, dict)]
    t_list += t_open
    code, out, err, t_ready = sh([bd(), "ready", "--json", "--limit", "0"], cwd=repo)
    ready = [r["id"] for r in json.loads(out or "[]") if isinstance(r, dict)] if code == 0 else None
    beads = [slim(r) for r in rows if isinstance(r, dict) and r.get("id")]
    data.update(state="ok", vision=vision(repo), beads=beads, ready=ready)
    if ready is None:
        data["ready_error"] = "bd ready exit %d: %s" % (code, first_line(err))
    # The page redraws only when this changes, so a refresh that found nothing new keeps its scroll.
    data["digest"] = hashlib.sha1(json.dumps([beads, ready, data["vision"]], sort_keys=True).encode()).hexdigest()
    log("%s: %d beads, bd list %.1fs, bd ready %.1fs" % (repo, len(beads), t_list, t_ready))
    return data


def one_bead(bead):
    """One bead's full record, for the page's side panel."""
    repo = main_checkout(os.environ.get("WHEEL_SPACE_DIR") or os.getcwd())
    if not re.match(r"^[A-Za-z0-9][A-Za-z0-9._-]*$", bead or ""):
        return {"id": bead, "error": "not a bead id: %r" % bead}
    code, out, err, took = sh([bd(), "show", bead, "--json", "--include-comments", "--brief-deps"], cwd=repo, timeout=120)
    log("%s: bd show %s exit %d, %.1fs" % (repo, bead, code, took))
    if code != 0:
        return {"id": bead, "error": "bd show exit %d: %s" % (code, first_line(err))}
    r = json.loads(out)
    r = r[0] if isinstance(r, list) and r else r
    if not isinstance(r, dict) or not r:
        return {"id": bead, "error": "bd show %s returned no bead" % bead}
    for k in LONG:
        if r.get(k):
            r[k] = inline_images(r[k], repo)
    for c in r.get("comments") or []:
        for k in ("text", "body"):
            if c.get(k):
                c[k] = inline_images(c[k], repo)
    return {"id": bead, "record": r}


def prune_runs(keep=RUNS_KEPT):
    """Remove all but the newest `keep` run logs in the pane's runs/ folder: each holds one bead's
    record, images included, and only this page runs commands in it."""
    d = os.path.join(os.environ.get("WHEEL_DATA_DIR") or "", "runs")
    if not os.environ.get("WHEEL_DATA_DIR") or not os.path.isdir(d):
        return
    logs = sorted((f for f in os.listdir(d) if f.endswith(".log")), key=lambda f: os.path.getmtime(os.path.join(d, f)))
    for f in logs[:-keep]:
        try:
            os.remove(os.path.join(d, f))
        except OSError:
            pass


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--bead":
        # One line on stdout, after the log line on stderr: the page reads the run's log and parses
        # its last line.
        try:
            out = one_bead(sys.argv[2])
        except Exception as e:
            out = {"id": sys.argv[2], "error": "%s: %s" % (type(e).__name__, e)}
        print(json.dumps(out))
        prune_runs()
        return
    space_dir = os.environ.get("WHEEL_SPACE_DIR") or os.getcwd()
    repo = main_checkout(space_dir)
    data = {"now_ms": int(time.time() * 1000), "space_dir": space_dir, "repo": repo,
            "name": os.path.basename(repo.rstrip("/")), "script": os.path.abspath(__file__)}
    try:
        snapshot(data)
    except Exception as e:  # the page shows the failure instead of the last good snapshot
        data.update(state="error", error="%s: %s" % (type(e).__name__, e))
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
