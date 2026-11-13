#!/usr/bin/env bash
# § codex-hooks: Wheel's Codex hook hashes still match what Codex computes.
#
# Run it by name; all.sh does not. It needs codex on PATH but no Codex login: it starts wheeld
# (which writes the `-c` values it hands Codex, with a trusted_hash per hook), starts
# `codex app-server` in a Codex home of its own with those values, calls `hooks/list`, and fails
# when any generated hook is missing from the answer, its currentHash differs from the generated
# trusted_hash, or its trustStatus is not trusted.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

command -v codex >/dev/null || { echo "needs codex on PATH" >&2; exit 2; }
command -v python3 >/dev/null || { echo "needs python3" >&2; exit 2; }

start_daemon
ARGS="$INST/shell/hooks/codex-hooks.args"
poll 10 test -s "$ARGS" && ok "wheeld wrote $ARGS" || { fail "wheeld wrote codex-hooks.args" "missing or empty: $ARGS"; finish; }

export CODEX_HOME="$SCR/codex-home"
mkdir -p "$CODEX_HOME" "$SCR/proj"
echo "codex: $(codex --version)"

out="$(python3 -I - "$ARGS" "$SCR/proj" <<'PY' 2>&1
import json, re, subprocess, sys, threading

args_file, cwd = sys.argv[1], sys.argv[2]
lines = [l for l in open(args_file).read().split("\n") if l]
cmd = ["codex"]
for l in lines:
    cmd += ["-c", l]
cmd += ["app-server"]

state = next((l for l in lines if l.startswith("hooks.state=")), "")
want = dict(re.findall(r'"([^"]+)"=\{trusted_hash="(sha256:[0-9a-f]+)"\}', state))
if not want:
    print("FAIL no trusted_hash in the generated hooks.state value")
    sys.exit(1)

p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
def send(msg):
    p.stdin.write(json.dumps(msg) + "\n"); p.stdin.flush()
result = {}
def reader():
    for line in p.stdout:
        try:
            m = json.loads(line)
        except ValueError:
            continue
        if m.get("id") == 2:
            result["m"] = m
            return
t = threading.Thread(target=reader, daemon=True)
t.start()
send({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "wheel-check", "title": "check", "version": "0"}}})
send({"method": "initialized"})
send({"id": 2, "method": "hooks/list", "params": {"cwds": [cwd]}})
t.join(30)
p.kill()
m = result.get("m")
if not m or "result" not in m:
    print("FAIL hooks/list gave no result: %s" % json.dumps(m))
    sys.exit(1)
listed = {h["key"]: h for e in m["result"]["data"] for h in e["hooks"]}
bad = 0
for key, h in sorted(want.items()):
    got = listed.get(key)
    if got is None:
        print("FAIL %s: not listed by Codex" % key); bad += 1; continue
    problems = []
    if got["currentHash"] != h:
        problems.append("currentHash %s, generated %s" % (got["currentHash"], h))
    if got["trustStatus"] != "trusted":
        problems.append("trustStatus %s" % got["trustStatus"])
    if problems:
        print("FAIL %s: %s" % (key, "; ".join(problems))); bad += 1
    else:
        print("ok   %s: currentHash %s, trusted" % (key, h))
sys.exit(1 if bad else 0)
PY
)"
rc=$?
echo "$out"
if [ "$rc" = 0 ]; then PASS=$((PASS + $(echo "$out" | grep -c '^ok'))); else FAIL=$((FAIL + 1)); fi
finish
