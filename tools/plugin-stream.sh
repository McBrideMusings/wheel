#!/bin/bash
# Check plugin video streams (ADR-0053, wheel-xy1y.5) against a throwaway wheeld: the gallery plugin's backend
# (plugins/gallery/backend.py) lists the stream `demo` in `initialize`, `wheel plugin stream` gets its socket as a
# VideoStream asks for it, a reader on that socket gets the parameter sets, video and audio frames, and an input frame
# sent back reaches the backend (its `gallery.input` feed). Refusals: a stream outside the pane's reads, one the backend
# does not list, a name that is not <plugin>.<name>. Prints PASS or FAIL per check. Uses no GUI, focus, mouse or keyboard;
# everything it starts is killed by pid at the end.
#
#   tools/plugin-stream.sh            # uses target/debug/wheeld and target/debug/wheel
#   WHEELD=... WHEEL=... tools/plugin-stream.sh
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=/private/tmp/claude/wheel/ps
PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
FAILS=0
PIDS=()

pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }

cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null; done
  sleep 0.5
  for pid in "${PIDS[@]}"; do kill -9 "$pid" 2>/dev/null; done
  pkill -f "$T/.*backend.py" 2>/dev/null
}
trap cleanup EXIT

rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/proj/.wheel/plugins" "$T/wide/.wheel/plugins"
: > "$T/machine.toml"
cp -R "$ROOT/plugins/gallery" "$T/proj/.wheel/plugins/gallery"
# wide: the same gallery whose pane may read any gallery feed or stream, to reach a stream the backend does not list.
cp -R "$ROOT/plugins/gallery" "$T/wide/.wheel/plugins/gallery"
sed -i '' 's/^reads = .*/reads = ["gallery.*"]/' "$T/wide/.wheel/plugins/gallery/wheel-plugin.toml"

"$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
  --log-dir "$T/logs" --cli "$WHEEL" > "$T/wheeld.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }

# A gallery pane in a new space at $1: sets SPACE and PANE.
gallery_pane() {
  SPACE=$(w space new "$1")
  PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
  w pane kind "$PANE" plugin --plugin gallery --plugin-pane gallery > /dev/null
}
gallery_pane "$T/wide"
WIDE=$PANE
gallery_pane "$T/proj"

SOCKET=$(w plugin stream "$PANE" gallery.demo 2>&1)
case "$SOCKET" in
  "$T"/support/v/*/demo.sock) pass "plugin_stream answers the socket in the backend's stream folder ($SOCKET)" ;;
  *) fail "plugin_stream answered: $SOCKET" ;;
esac
STREAMS=$(w plugin ls --json --space "$SPACE" 2>&1 | python3 -c '
import json, sys
for p in json.load(sys.stdin):
    if p["id"] == "gallery":
        print(",".join(p.get("backend", {}).get("streams", [])))
')
[ "$STREAMS" = demo ] && pass "plugin ls lists the backend's streams" || fail "plugin ls streams: $STREAMS"


# Read the stream for 1 s: the first frame is H.264 parameter sets, then video and audio; send one key event back.
python3 -I - "$SOCKET" > "$T/read.out" 2>&1 <<'PYEOF'
import json, socket, struct, sys, time
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
for _ in range(50):
    try:
        s.connect(sys.argv[1]); break
    except OSError:
        time.sleep(0.1)
event = json.dumps({"kind": "down", "code": 0, "key": "a"}).encode()
s.sendall(struct.pack(">BI", 16, len(event)) + event)
buf, kinds, first, video, end = b"", [], None, 0, time.time() + 1
s.settimeout(1)
while time.time() < end:
    try:
        chunk = s.recv(65536)
    except socket.timeout:
        break
    if not chunk:
        break
    buf += chunk
    while len(buf) >= 5:
        kind, n = struct.unpack(">BI", buf[:5])
        if len(buf) < 5 + n:
            break
        payload, buf = buf[5:5 + n], buf[5 + n:]
        if first is None:
            first = (kind, payload[:1].hex())
        kinds.append(kind)
print(json.dumps({"first": first, "video": kinds.count(2), "audio": kinds.count(3)}))
PYEOF
READ=$(tail -1 "$T/read.out")
python3 -c 'import json,sys; r=json.loads(sys.argv[1]); sys.exit(0 if r["first"] == [1, "01"] else 1)' "$READ" 2>/dev/null \
  && pass "the first frame is H.264 parameter sets" || fail "first frame: $READ"
python3 -c 'import json,sys; r=json.loads(sys.argv[1]); sys.exit(0 if r["video"] >= 20 and r["audio"] >= 20 else 1)' "$READ" 2>/dev/null \
  && pass "1 s of the stream holds video and audio frames ($READ)" || fail "stream frames: $READ"
# The first update is the hub's value from before the backend answered; the second is the backend's.
INPUT=$(w plugin feed "$PANE" gallery.input --count 2 2>&1)
if echo "$INPUT" | tail -1 | python3 -c '
import json, sys
v = json.load(sys.stdin)["value"]
sys.exit(0 if v["count"] == 1 and json.loads(v["last"].split(" ", 1)[1]) == {"kind": "down", "code": 0, "key": "a"} else 1)
' 2>/dev/null; then
  pass "an input frame reaches the backend"
else
  fail "input: $INPUT"
fi

refused() {
  local name=$1 pane=$2 stream=$3 want=$4 got
  got=$(w plugin stream "$pane" "$stream" 2>&1) && { fail "$name: answered $got"; return; }
  case "$got" in *"$want"*) pass "$name" ;; *) fail "$name: $got" ;; esac
}
refused "a stream outside the pane's reads is refused" "$PANE" gallery.other "may not read gallery.other"
refused "a stream the backend does not list is refused" "$WIDE" gallery.nope "has no stream nope; its streams: demo"
refused "a stream name that is no file name is refused" "$WIDE" "gallery.a/b" "a stream name is letters, digits, _ and -"

echo "$([ "$FAILS" -eq 0 ] && echo PASS || echo FAIL) plugin-stream: $FAILS failed"
exit "$FAILS"
