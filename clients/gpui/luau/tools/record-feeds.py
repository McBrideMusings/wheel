"""Record the feeds the golden test asked for from the live wheeld, trimmed and scrubbed.

usage: python3 record-feeds.py <worktree> <asks-dir> <recorder-pane> <agent-pane> <space>
"""
import json, os, re, selectors, subprocess, sys, time

wt, asks_dir, pane, agent_pane, space = sys.argv[1:6]
scripts = os.path.join(wt, "clients/gpui/luau/tests/scripts")
cases = {c["name"]: c for c in json.load(open(os.path.join(scripts, "cases.json")))}
# Every other feed carries the person's beads, PRs, posts, agents or config, which the public repo must not hold.
PUBLIC = {"gallery.input"}
HOME = os.path.expanduser("~")


def live(v):
    if v == []:
        return {}  # an empty Luau table converts to []; the daemon wants params as an object
    s = json.dumps(v).replace("/Users/me", HOME).replace('"s1"', json.dumps(space)).replace('"p1"', json.dumps(agent_pane))
    return json.loads(s)


def scrub(v):
    s = json.dumps(v).replace(HOME, "/Users/me").replace(space, "s1").replace(agent_pane, "p1")
    return json.loads(s)


def trim(v, depth=0):
    if isinstance(v, list):
        return [trim(x, depth + 1) for x in v[:12]]
    if isinstance(v, dict):
        keys = list(v)[:30]
        return {k: trim(v[k], depth + 1) for k in keys}
    if isinstance(v, str) and len(v) > 240:
        return v[:240]
    return v


def blank(v):
    """Free text in a private feed becomes letters of the same case and length; ids and enums stay."""
    if isinstance(v, list):
        return [blank(x) for x in v]
    if isinstance(v, dict):
        return {k: blank(x) for k, x in v.items()}
    if isinstance(v, str) and (len(v) > 24 or re.search(r"\s", v)):
        return re.sub(r"[a-z]", "a", re.sub(r"[A-Z]", "A", v))
    return v


def read(feed, params, wait=60):
    p = subprocess.Popen(["wheel", "plugin", "feed", pane, feed, "--params", json.dumps(params)],
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    sel = selectors.DefaultSelector()
    sel.register(p.stdout, selectors.EVENT_READ)
    value, end = None, time.time() + wait
    while time.time() < end:
        if not sel.select(timeout=max(0.1, end - time.time())):
            break
        line = p.stdout.readline()
        if not line:
            break
        msg = json.loads(line)
        if msg.get("value") is not None:
            value = msg
            break
    p.kill()
    err = p.stderr.read().strip()
    if value is None and err:
        print(f"  {feed}: {err}", file=sys.stderr)
    return value


current = None
for fname in sorted(os.listdir(asks_dir)):
    name = fname[:-5]
    asks = json.load(open(os.path.join(asks_dir, fname)))
    if not asks:
        continue
    case = cases[name]
    target = (case["plugin"], case["plugin_pane"])
    if target != current:
        subprocess.run(["wheel", "pane", "kind", pane, "plugin", "--plugin", target[0], "--plugin-pane", target[1]], check=True)
        current = target
        time.sleep(1)
    path = os.path.join(scripts, f"{name}.feeds.json")
    feeds = json.load(open(path)) if os.path.exists(path) else []
    for ask in asks:
        msg = read(ask["name"], live(ask["params"]))
        value = trim(msg["value"]) if msg else None
        if value is not None and ask["name"] not in PUBLIC:
            value = blank(value)
        feeds.append({"name": ask["name"], "params": ask["params"], "value": scrub(value), "stale": False})
        print(f"{name}: {ask['name']} {json.dumps(ask['params'])} -> {len(json.dumps(value))} bytes")
    json.dump(feeds, open(path, "w"), indent=1, sort_keys=True)
    open(path, "a").write("\n")
