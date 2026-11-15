#!/bin/bash
# Check the kennel plugin package (plugins/kennel) against a throwaway wheeld and a copy of kennel's config
# (KENNEL_CONFIG_DIR), so the machine's real routes are never touched: its feeds and its route actions, through
# `wheel plugin feed` and `wheel plugin action` as its pane. Prints PASS or FAIL per check. Uses no GUI, focus, mouse
# or keyboard. Everything it starts is killed by pid at the end. Needs `kennel` on PATH and a config to copy.
#
#   tools/kennel-plugin.sh            # uses target/debug/wheeld, wheel and wheel-runner
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=/private/tmp/claude/wheel/kplug
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

command -v kennel > /dev/null || { echo "FAIL kennel is not on PATH"; exit 1; }
rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/proj/.wheel/plugins" "$T/kennel" "$T/routed"
: > "$T/machine.toml"
cp "${KENNEL_CONFIG_DIR:-$HOME/.config/kennel}/config.toml" "$T/kennel/"
: > "$T/kennel/routes"
export KENNEL_CONFIG_DIR="$T/kennel"
ln -s "$ROOT/plugins/kennel" "$T/proj/.wheel/plugins/kennel"
PROFILE=$(kennel route list | awk '/^Profiles/ {p = 1; next} p {print $1; exit}')

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
w pane kind "$PANE" plugin --plugin kennel --plugin-pane kennel > /dev/null
echo "space $SPACE pane $PANE profile $PROFILE"

# The last value a feed sends within its first updates (the first may be null, before the first run ends).
feed() { w plugin feed "$PANE" "$1" --params "$2" --count "${3:-2}" 2> /dev/null | tail -1; }
D="{\"dir\":\"$T/routed\"}"

check "the [commands] table gives the package wheel-runner commands" "$(w plugin ls --json --space "$SPACE" | python3 -c '
import json, sys
p = next(p for p in json.load(sys.stdin) if p["id"] == "kennel")
print("ok" if (p.get("backend") or {}).get("command") == "wheel-runner commands" else p)')"

feed kennel.routes "$D" > "$T/routes0.json"
check "routes: kennel route list's lines, no route yet" "$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]["value"]
print("ok" if v[0].startswith("Routes (") and "  (none)" in v else v)' "$T/routes0.json")"

feed kennel.which "$D" > "$T/which.json"
check "which: kennel which in the pane's dir" "$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]["value"]
print("ok" if v[0] == "profile\t(no route)" else v)' "$T/which.json")"

feed kennel.harness '{"section":"hooks","scope":"global","harness":"claude"}' > "$T/harness.json"
check "harness: kennel harness show's JSON" "$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]["value"]
print("ok" if v["section"] == "hooks" and v["scope"]["kind"] == "global" and "blocks" in v else v)' "$T/harness.json")"

feed kennel.doctor '{}' > "$T/doctor.json"
check "doctor: its lines, whatever its exit code" "$(python3 -c '
import json, sys
f = json.load(open(sys.argv[1]))["value"]
print("ok" if f["error"] is None and any(l.startswith("Config") for l in f["value"]) else f)' "$T/doctor.json")"

w plugin action "$PANE" kennel.route_add --args "{\"path\":\"$T/routed\",\"profile\":\"$PROFILE\",\"permanent\":true,\"dir\":\"$T/routed\"}" > "$T/add.json"
check "route_add: exit 0 and the route in kennel route list" "$(python3 -c '
import json, sys
r = json.load(open(sys.argv[1]))
print("ok" if r["code"] == 0 and sys.argv[2] in open(sys.argv[3]).read() else r)' "$T/add.json" "$T/routed" "$T/kennel/routes")"

feed kennel.which "$D" > "$T/which2.json"
check "which: the routed dir now gets the profile" "$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]["value"]
print("ok" if v[0] == "profile\t" + sys.argv[2] else v)' "$T/which2.json" "$PROFILE")"

w plugin action "$PANE" kennel.route_add --args "{\"path\":\"$T/routed\",\"profile\":\"no such profile\"}" > "$T/bad.json"
check "route_add with an unknown profile: exit 1 and kennel's message" "$(python3 -c '
import json, sys
r = json.load(open(sys.argv[1]))
print("ok" if r["code"] == 1 and "no [profiles.no such profile]" in r["stderr"] else r)' "$T/bad.json")"

w plugin action "$PANE" kennel.route_rm --args "{\"path\":\"$T/routed\"}" > "$T/rm.json"
check "route_rm: exit 0 and the route gone" "$(python3 -c '
import json, sys
r = json.load(open(sys.argv[1]))
print("ok" if r["code"] == 0 and sys.argv[2] not in open(sys.argv[3]).read() else r)' "$T/rm.json" "$T/routed" "$T/kennel/routes")"

w plugin action "$PANE" kennel.route_prune --args '{}' > "$T/prune.json"
check "route_prune: exit 0, nothing expired" "$(python3 -c '
import json, sys
r = json.load(open(sys.argv[1]))
print("ok" if r["code"] == 0 and r["stdout"].strip() == "nothing expired" else r)' "$T/prune.json")"

echo "fails: $FAILS"
[ "$FAILS" -eq 0 ]
