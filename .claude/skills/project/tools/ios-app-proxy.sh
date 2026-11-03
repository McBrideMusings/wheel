#!/usr/bin/env bash
# § Web-server app on the phone (wheel-t5yu): the pane of an `app` entry shows the program's page, and
# its event stream keeps updating it, through the remote listener. Shares ios-web.sh's setup.
# § iOS web pane (wheel-20eq): a web pane on the phone loads its artifact from the Mac's remote
# listener at a non-loopback address. App Transport Security refuses URLSession https there
# (-1200); PageSchemeHandler fetches through PinnedHTTP (Network.framework) instead, with the
# certificate pin as the whole trust check.
#
# Needs WHEEL_IOS_APP: the built Wheel.app for the simulator (WheelPhone scheme), and a pool
# simulator. WHEEL_IOS_HOST (default: the Mac's first non-loopback IPv4 address) is the address the
# phone dials; the listener binds 0.0.0.0.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SIM=~/.claude/tools/sim
BUNDLE=com.piercemakes.wheel.phone
HOST="${WHEEL_IOS_HOST:-$(ifconfig | awk '/inet / && $2 != "127.0.0.1" { print $2; exit }')}"
UDID=""
ios_cleanup() { [ -n "$UDID" ] && "$SIM" release >/dev/null 2>&1; cleanup; }
trap ios_cleanup EXIT

[ -d "${WHEEL_IOS_APP:-}" ] || { echo "set WHEEL_IOS_APP to the built Wheel.app (WheelPhone, iphonesimulator)" >&2; exit 2; }
[ -n "$HOST" ] || { echo "no non-loopback address; set WHEEL_IOS_HOST" >&2; exit 2; }
case "$HOST" in 127.*) echo "WHEEL_IOS_HOST must not be loopback: ATS only refuses other addresses" >&2; exit 2 ;; esac

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "0.0.0.0:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

SITE="$SCR/proj"
mkdir -p "$SITE"
cat >"$SITE/server.py" <<'PY'
import http.server, os, time
PAGE = b"""<!doctype html><title>live</title><body><h1>live-app</h1><p id=n>ticks: 0</p>
<script>new EventSource("events").onmessage = e => document.getElementById("n").textContent = "ticks: " + e.data</script>"""
class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass
    def do_GET(self):
        if self.path == "/events":
            self.send_response(200); self.send_header("Content-Type", "text/event-stream"); self.end_headers()
            for i in range(1, 600):
                self.wfile.write(("data: %d\n\n" % i).encode()); self.wfile.flush(); time.sleep(0.3)
        else:
            self.send_response(200); self.send_header("Content-Type", "text/html"); self.send_header("Content-Length", str(len(PAGE))); self.end_headers(); self.wfile.write(PAGE)
s = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
os.write(3, (str(s.server_address[1]) + "\n").encode())
s.serve_forever()
PY
printf '[artifacts.live]\napp = "exec python3 %s/server.py"\n' "$SITE" >"$SITE/wheel.toml"
SID="$(w space new "$SITE")"
PANE="$(w space ls --json | jget 0.panes.0)"
w pane kind "$PANE" web --artifact live >/dev/null

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
xcrun simctl install "$UDID" "$WHEEL_IOS_APP"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
PLOG="$DATA/Documents/phone.log"
rm -f "$PLOG"
plog() { grep -q -- "$1" "$PLOG" 2>/dev/null; }

code="$(w remote pair --address "$HOST")"
SIMCTL_CHILD_WHEEL_PAIR_JSON="$code" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
check "the phone pairs at $HOST" poll 40 plog "connected to wss://$HOST:$RPORT/ws"
check "the program's page loads with 200" poll 30 plog "web pane $PANE page wheel-page://pages/artifacts/$PANE/ -> 200"
check "the page finishes loading" poll 30 plog "web pane $PANE loaded, status 200"
check "the program's event stream is handed over as it arrives" poll 30 plog "web pane $PANE page wheel-page://pages/artifacts/$PANE/events -> 200, streaming"
sleep 3
[ -z "${WHEEL_SHOTS:-}" ] || xcrun simctl io "$UDID" screenshot "$WHEEL_SHOTS/ios-app-proxy-page.png" >/dev/null 2>&1
[ "$FAIL" -eq 0 ] || { echo "--- phone.log"; tail -40 "$PLOG"; echo "--- wheeld.log"; grep -E "artifact|remote|refused|app" "$(LOG)" | tail -20 | cut -c1-300; }
finish
