#!/bin/bash
# Check wheeld's server identity (wheel-2aln.2, ADR-0057) against a throwaway wheeld: status.host
# carries id, name, color and platform; the id survives a restart; deleting server-id gives a new
# one; server.name and server.color show after `wheel config set`. Prints PASS or FAIL per check.
# Uses no GUI; kills what it starts by pid.
#
#   tools/server-identity.sh        # uses target/debug/wheeld and target/debug/wheel
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=/private/tmp/claude/wheel/server-identity
PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
FAILS=0
PID=
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }
stop() { [ -n "$PID" ] && kill "$PID" 2>/dev/null && wait "$PID" 2>/dev/null; PID=; }
trap stop EXIT

start() {
  "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
    --log-dir "$T/logs" --cli "$WHEEL" >> "$T/wheeld.out" 2>&1 &
  PID=$!
  for _ in $(seq 1 100); do
    [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && return
    sleep 0.2
  done
}
w() { env -u WHEEL_PANE -u WHEEL_SPACE "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }
host() { w status --json | python3 -c "import json,sys; print(json.load(sys.stdin)['host'].get('$1',''))"; }

rm -rf "$T"
mkdir -p "$T/support" "$T/logs"
: > "$T/machine.toml"
start
ID=$(host id)
[[ "$ID" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$ ]] && pass "status.host.id is a UUID ($ID)" || fail "status.host.id is a UUID (got '$ID')"
[ -n "$ID" ] && [ "$(cat "$T/support/server-id")" = "$ID" ] && pass "server-id holds the id" || fail "server-id holds the id"
[ "$(host name)" = "$(hostname)" ] && pass "name defaults to the hostname ($(hostname))" || fail "name defaults to the hostname (got '$(host name)')"
C=$(host color)
[ -n "$C" ] && pass "color defaults to one picked by the id ($C)" || fail "color has a default"
[ "$(host platform)" = "darwin_$(uname -m | sed 's/x86_64/amd64/')" ] && pass "platform ($(host platform))" || fail "platform (got '$(host platform)')"

stop; start
[ -n "$ID" ] && [ "$(host id)" = "$ID" ] && pass "a restart keeps the id" || fail "a restart keeps the id (got '$(host id)')"

w config set --machine server.name "Studio Mac" > /dev/null && w config set --machine server.color teal > /dev/null
[ "$(host name)" = "Studio Mac" ] && pass "server.name shows in status" || fail "server.name shows in status (got '$(host name)')"
[ "$(host color)" = "teal" ] && pass "server.color shows in status" || fail "server.color shows in status (got '$(host color)')"
w config set --machine server.color magenta > /dev/null 2>&1 && fail "server.color refuses magenta" || pass "server.color refuses a colour that is not a system colour"

stop; rm "$T/support/server-id"; start
NEW=$(host id)
[ -n "$NEW" ] && [ "$NEW" != "$ID" ] && pass "deleting server-id gives a new id" || fail "deleting server-id gives a new id (got '$NEW')"
w remote info | grep -qx "server: $NEW" && pass "remote info prints the id" || fail "remote info prints the id"

[ "$FAILS" -eq 0 ] && echo "ALL PASS" || { echo "$FAILS FAILED"; exit 1; }
