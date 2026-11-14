#!/bin/bash
# Check the TypeScript SDK (wheel-pmf2.3.6): the ticker example (sdk/ts/examples/ticker) is a plugin
# whose backend is `node` on the SDK; its feeds reach a plugin pane through a throwaway wheeld as
# `wheel plugin feed` reads them, its action answers, and a failing action returns the error.
# The Rust SDK's check is `CLOCK_BACKEND=rust tools/plugin-backend.sh` and `tools/plugin-call.sh`.
# Prints PASS or FAIL per check. Uses no GUI, focus, mouse or keyboard; kills what it starts by pid.
#
#   tools/plugin-sdk.sh            # uses target/debug/wheeld and target/debug/wheel
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=/private/tmp/claude/wheel/sdk-ts
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
}
trap cleanup EXIT

( cd "$ROOT/sdk/ts" && npm install --no-audit --no-fund > "$T.npm.log" 2>&1 && npm run build >> "$T.npm.log" 2>&1 ) \
  && pass "the SDK installs and compiles with tsc" || { fail "the SDK installs and compiles: $(tail -5 "$T.npm.log")"; exit 1; }

rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/proj/.wheel/plugins"
: > "$T/machine.toml"
ln -s "$ROOT/sdk/ts/examples/ticker" "$T/proj/.wheel/plugins/ticker"

"$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
  --log-dir "$T/logs" --cli "$WHEEL" > "$T/wheeld.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }

SPACE=$(w space new "$T/proj")
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
w pane kind "$PANE" plugin --plugin ticker --plugin-pane main > /dev/null

w plugin feed "$PANE" ticker.tick --count 4 > "$T/tick.jsonl" 2> "$T/tick.err"
python3 - "$T/tick.jsonl" <<'EOF' > "$T/tick.verdict"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
ns = [r["value"]["n"] for r in rows if r.get("value")]
print("ok" if len(ns) >= 3 and all(b - a == 1 for a, b in zip(ns, ns[1:])) else "ns=%r rows=%d" % (ns, len(rows)))
EOF
check "ticker.tick arrives from the TypeScript backend with n counting up by one" "$(cat "$T/tick.verdict")"

w plugin feed "$PANE" ticker.log --count 5 > "$T/log.jsonl" 2> "$T/log.err"
python3 - "$T/log.jsonl" <<'EOF' > "$T/log.verdict"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
rows = rows[1:] if rows and rows[0].get("value") is None else rows
first, rest = rows[0], rows[1:]
ok = first.get("value") is not None and "patch" not in first and len(rest) >= 2 and all(r.get("patch") and "value" not in r for r in rest)
print("ok" if ok else "rows=%r" % rows)
EOF
check "ticker.log is a value then patches" "$(cat "$T/log.verdict")"

w plugin action "$PANE" ticker.echo --args '{"n":7}' > "$T/echo.out" 2>&1
python3 - "$T/echo.out" <<'EOF' > "$T/echo.verdict"
import json, sys
text = open(sys.argv[1]).read()
v = json.loads(text[text.index("{"):])
v = v.get("result", v)
print("ok" if v["echo"] == {"n": 7} and v["caller"].get("plugin") == "ticker" else "out=%r" % text)
EOF
check "ticker.echo returns its args and the caller" "$(cat "$T/echo.verdict")"

w plugin action "$PANE" ticker.fail > "$T/fail.out" 2>&1
grep -q "the ticker refuses" "$T/fail.out" && pass "a thrown ActionError reaches the caller as an error" \
  || fail "a thrown ActionError reaches the caller as an error: $(cat "$T/fail.out")"

w plugin logs ticker --space "$SPACE" > "$T/logs.txt" 2>&1
grep -qE 'info initialized for ticker' "$T/logs.txt" && pass "the SDK's initialize handler logs through the log notification" \
  || fail "log notification: $(cat "$T/logs.txt")"

[ "$FAILS" = 0 ] && echo "all checks passed" || { echo "$FAILS check(s) failed"; exit 1; }
