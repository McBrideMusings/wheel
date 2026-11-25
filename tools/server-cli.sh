#!/bin/bash
# Check `wheel server` and the global `--server` (wheel-2aln.6, ADR-0057) against two throwaway
# wheelds, each with its remote listener on 127.0.0.1: `server add` pairs with A's code and records
# its id; `--server` reaches A's spaces and not B's; a renamed label still resolves by id; disable
# refuses; an address that now leads to B is refused naming both ids; `remove` forgets it. The list
# goes to a scratch file (WHEEL_SERVERS_FILE); the credential goes to the login Keychain under
# "Wheel Server" and the scratch server's id, and `remove` takes it out. Prints PASS or FAIL per
# check. Uses no GUI; kills what it starts by pid.
#
#   tools/server-cli.sh        # uses target/debug/wheeld and target/debug/wheel
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=/private/tmp/claude/wheel/server-cli
FAILS=0
PIDS=()
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }
port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
cleanup() {
  [ -n "${A_ID:-}" ] && security delete-generic-password -s "Wheel Server" -a "$A_ID" > /dev/null 2>&1
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null && wait "$p" 2>/dev/null; done
}
trap cleanup EXIT

# start <dir> <local port> <remote port>
start() {
  mkdir -p "$1/support" "$1/logs"
  printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$3" > "$1/machine.toml"
  "$WHEELD" --listen "127.0.0.1:$2" --support-dir "$1/support" --machine-file "$1/machine.toml" \
    --log-dir "$1/logs" --cli "$WHEEL" >> "$1/wheeld.out" 2>&1 &
  PIDS+=($!)
  for _ in $(seq 1 100); do
    [ -f "$1/support/token" ] && nc -z 127.0.0.1 "$3" > /dev/null 2>&1 && return
    sleep 0.2
  done
  echo "wheeld in $1 never listened on $3"
}
# local <dir> <port> <args...>: a command on that wheeld's local listener.
local_w() { local d=$1 p=$2; shift 2; env -u WHEEL_PANE -u WHEEL_SPACE "$WHEEL" --url "ws://127.0.0.1:$p/ws" --token-file "$d/support/token" "$@"; }
# w <args...>: a command that names its server with --server, from inside a pane's environment.
w() { env WHEEL_SERVERS_FILE="$T/servers.toml" WHEEL_PANE=p999999 "$WHEEL" "$@"; }
id_of() { local_w "$1" "$2" status --json | python3 -c "import json,sys; print(json.load(sys.stdin)['host']['id'])"; }

rm -rf "$T"
mkdir -p "$T"
AL=$(port); AR=$(port); BL=$(port); BR=$(port)
start "$T/a" "$AL" "$AR"
start "$T/b" "$BL" "$BR"
A_ID=$(id_of "$T/a" "$AL")
B_ID=$(id_of "$T/b" "$BL")
local_w "$T/a" "$AL" space new "$T" --name only-on-a > /dev/null
local_w "$T/b" "$BL" space new "$T" --name only-on-b > /dev/null

CODE=$(local_w "$T/a" "$AL" remote pair --address 127.0.0.1)
OUT=$(w server add "$CODE" --label dev 2>&1)
echo "$OUT" | grep -q "paired with dev ($A_ID)" && pass "server add pairs and records the id ($A_ID)" || fail "server add: $OUT"
grep -q "id = \"$A_ID\"" "$T/servers.toml" && pass "servers.toml holds the id" || fail "servers.toml holds the id"
grep -qi "credential" "$T/servers.toml" && fail "servers.toml holds no credential" || pass "servers.toml holds no credential"
security find-generic-password -s "Wheel Server" -a "$A_ID" > /dev/null 2>&1 && pass "the credential is in the Keychain" || fail "the credential is in the Keychain"
local_w "$T/a" "$AL" remote devices | grep -q "wheel CLI on" && pass "A lists the CLI as a device" || fail "A lists the CLI as a device"

SPACES=$(w --server dev space ls 2>&1)
echo "$SPACES" | grep -q only-on-a && ! echo "$SPACES" | grep -q only-on-b && pass "--server dev space ls lists A's spaces" || fail "--server dev space ls: $SPACES"
w --server "$A_ID" space ls 2>&1 | grep -q only-on-a && pass "--server takes the id" || fail "--server takes the id"
OUT=$(w --server dev pane near 2>&1)
echo "$OUT" | grep -q p999999 && fail "--server ignores this machine's WHEEL_PANE: $OUT" || pass "--server ignores this machine's WHEEL_PANE"

w server rename dev box > /dev/null
w --server box space ls 2>&1 | grep -q only-on-a && pass "the new label resolves" || fail "the new label resolves"
w --server "$A_ID" space ls 2>&1 | grep -q only-on-a && pass "a renamed server still resolves by id" || fail "a renamed server still resolves by id"
w server rename box local > /dev/null 2>&1 && fail "the label local is refused" || pass "the label local is refused"

STATUS=$(w server status --url "ws://127.0.0.1:$AL/ws" --token-file "$T/a/support/token" 2>&1)
echo "$STATUS" | grep -q "^box .*ok:" && pass "server status reports box ok" || fail "server status: $STATUS"

w server disable box > /dev/null
w --server box space ls > /dev/null 2>&1 && fail "a disabled server is refused" || pass "a disabled server is refused"
w server enable box > /dev/null

# A wheeld that lost its server-id (reinstalled, its support folder wiped) answers at the same
# address with the same certificate and devices, and a new id: only the id tells it apart.
kill "${PIDS[0]}" 2>/dev/null; wait "${PIDS[0]}" 2>/dev/null
rm "$T/a/support/server-id"
start "$T/a" "$AL" "$AR"
NEW_ID=$(id_of "$T/a" "$AL")
OUT=$(w --server box space ls 2>&1)
echo "$OUT" | grep -q "another wheeld" && echo "$OUT" | grep -q "$A_ID" && echo "$OUT" | grep -q "$NEW_ID" && pass "a server answering with another id is refused, naming both" || fail "another id: $OUT"
echo "$OUT" | grep -q only-on-a && fail "the refused server's spaces are not listed" || pass "the refused server's spaces are not listed"

w server remove box > /dev/null
security find-generic-password -s "Wheel Server" -a "$A_ID" > /dev/null 2>&1 && fail "remove deletes the credential" || pass "remove deletes the credential"
w server list | grep -q box && fail "remove drops the entry" || pass "remove drops the entry"

[ "$FAILS" -eq 0 ] && echo "ALL PASS" || { echo "$FAILS FAILED"; exit 1; }
