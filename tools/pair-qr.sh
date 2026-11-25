#!/bin/bash
# Check headless pairing (wheel-2aln.5) against a throwaway wheeld with its remote listener on
# 127.0.0.1: `wheeld pair` asks the running wheeld for a code and prints it as JSON and a QR;
# `wheel server add` pairs with that code; `wheel remote pair --qr` prints the same shape; with the
# listener off `wheeld pair` says so. The paired server goes to a scratch list (WHEEL_SERVERS_FILE)
# and its credential to the login Keychain, removed on exit. Prints PASS or FAIL per check. Uses no
# GUI; kills what it starts by pid.
#
#   tools/pair-qr.sh        # uses target/debug/wheeld and target/debug/wheel
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=/private/tmp/claude/wheel/pair-qr
FAILS=0
PIDS=()
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }
port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
cleanup() {
  [ -n "${ID:-}" ] && security delete-generic-password -s "Wheel Server" -a "$ID" > /dev/null 2>&1
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null && wait "$p" 2>/dev/null; done
}
trap cleanup EXIT

# start <dir> <local port> [remote port]: no remote port leaves remote access off.
start() {
  mkdir -p "$1/support" "$1/logs"
  if [ -n "${3:-}" ]; then printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$3" > "$1/machine.toml"; else : > "$1/machine.toml"; fi
  "$WHEELD" --listen "127.0.0.1:$2" --support-dir "$1/support" --machine-file "$1/machine.toml" \
    --log-dir "$1/logs" --cli "$WHEEL" >> "$1/wheeld.out" 2>&1 &
  PIDS+=($!)
  for _ in $(seq 1 100); do
    [ -f "$1/support/token" ] && nc -z 127.0.0.1 "${3:-$2}" > /dev/null 2>&1 && return
    sleep 0.2
  done
  echo "wheeld in $1 never listened"
}

rm -rf "$T"
mkdir -p "$T"
AL=$(port); AR=$(port); OL=$(port)
start "$T/a" "$AL" "$AR"
start "$T/off" "$OL"
ID=$(env -u WHEEL_PANE "$WHEEL" --url "ws://127.0.0.1:$AL/ws" --token-file "$T/a/support/token" status --json | python3 -c "import json,sys; print(json.load(sys.stdin)['host']['id'])")

OUT=$("$WHEELD" --listen "127.0.0.1:$AL" --support-dir "$T/a/support" pair --address 127.0.0.1 2>&1)
CODE=$(echo "$OUT" | head -1)
echo "$CODE" | python3 -c "import json,sys; c=json.load(sys.stdin); assert c['url']=='wss://127.0.0.1:$AR/ws' and len(c['cert_sha256'])==64 and c['token']" 2>/dev/null \
  && pass "wheeld pair prints the code: $CODE" || fail "wheeld pair code: $OUT"
ROWS=$(echo "$OUT" | tail -n +2 | grep -c $'\x1b\[30;107m')
[ "$ROWS" -ge 15 ] && pass "wheeld pair prints a QR of $ROWS rows" || fail "wheeld pair QR rows: $ROWS"
echo "$OUT" | tail -n +2 | grep -q '[█▀▄]' && pass "the QR is drawn in half blocks" || fail "the QR is drawn in half blocks"

ADD=$(env WHEEL_SERVERS_FILE="$T/servers.toml" "$WHEEL" server add "$CODE" --label a 2>&1)
echo "$ADD" | grep -q "paired with a ($ID)" && pass "wheel server add pairs with wheeld pair's code" || fail "server add: $ADD"

OUT=$(env -u WHEEL_PANE "$WHEEL" --url "ws://127.0.0.1:$AL/ws" --token-file "$T/a/support/token" remote pair --address 127.0.0.1 --qr 2>&1)
echo "$OUT" | head -1 | grep -q '"url":"wss://127.0.0.1:' && [ "$(echo "$OUT" | grep -c $'\x1b\[30;107m')" -ge 15 ] \
  && pass "wheel remote pair --qr prints the code and a QR" || fail "wheel remote pair --qr: $OUT"
OUT=$(env -u WHEEL_PANE "$WHEEL" --url "ws://127.0.0.1:$AL/ws" --token-file "$T/a/support/token" remote pair --address 127.0.0.1 2>&1)
[ "$(echo "$OUT" | wc -l | tr -d ' ')" = 1 ] && pass "wheel remote pair without --qr prints one line" || fail "wheel remote pair: $OUT"

OUT=$("$WHEELD" --listen "127.0.0.1:$OL" --support-dir "$T/off/support" pair 2>&1)
[ $? -ne 0 ] && echo "$OUT" | grep -q "remote access is off" && pass "wheeld pair with remote access off: $OUT" || fail "remote off: $OUT"
OUT=$("$WHEELD" --listen "127.0.0.1:$AL" --support-dir "$T/a/support" pair --address 10.9.9.9 2>&1)
[ $? -ne 0 ] && echo "$OUT" | grep -q "not an address wheeld listens on" && pass "an unknown --address is refused" || fail "unknown address: $OUT"

[ "$FAILS" = 0 ] && echo "all passed" || echo "$FAILS failed"
exit "$FAILS"
