#!/usr/bin/env bash
# § Web-server app in a pane (wheel-t5yu): an `app` entry whose program serves HTTP shows in a web
# pane of the Mac app through the daemon's proxy. A page that streams server-sent events updates
# without a reload, the pane's text equals a direct load of the program's own address, and a
# stopped app shows a page saying it is not running. Needs `make build-client` and jq. With
# WHEEL_SHOTS set, keeps app-proxy-<page|stopped>.png.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
PROJ="$SCR/proj"
mkdir -p "$PROJ"
cat >"$PROJ/server.py" <<'PY'
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
printf '[artifacts.live]\napp = "exec python3 %s/server.py"\n' "$PROJ" >"$PROJ/wheel.toml"
w space new "$PROJ" >/dev/null
launch_app
wait_st '.connection == "connected"' 20
P="$(w space ls | jget spaces.0.root.id)"
[ -z "${WHEEL_SHOTS:-}" ] || app_post "/window/front" >/dev/null
shot() { [ -z "${WHEEL_SHOTS:-}" ] || { sleep 2; w pane shot "$P" "$WHEEL_SHOTS/app-proxy-$1.png" >/dev/null; }; }
eval_js() { curl -s -X POST -H "Authorization: Bearer $(TOKEN)" -d "$(jq -cn --arg pane "$P" --arg js "$1" '{pane: $pane, js: $js}')" "http://127.0.0.1:$DEBUG_PORT/web/eval" | jq -r .result; }

expect_exit 0 "a pane shows the app artifact" w pane kind "$P" web --artifact live
APORT=""
for _ in $(seq 1 50); do APORT="$(w artifact app status "$P" --json 2>/dev/null | jq -r '.port // empty')"; [ -n "$APORT" ] && break; sleep 0.2; done
check "the app announces a port" test -n "$APORT"
check "the pane draws the program's page" poll 20 sh -c "[ \"\$(curl -s -X POST -H 'Authorization: Bearer $(TOKEN)' -d '{\"pane\":\"$P\",\"js\":\"document.querySelector(\\\"h1\\\").textContent\"}' http://127.0.0.1:$DEBUG_PORT/web/eval)\" = '{\"result\":\"live-app\"}' ]"
first="$(eval_js 'document.getElementById("n").textContent')"
sleep 2
second="$(eval_js 'document.getElementById("n").textContent')"
check "the event stream updates the pane without a reload ($first, then $second)" test "$first" != "$second" -a "$second" != "ticks: 0"
eq "the pane's text equals a direct load of the program's address" "$(eval_js 'document.querySelector("h1").textContent')" "$(curl -s "http://127.0.0.1:$APORT/" | sed -n 's/.*<h1>\(.*\)<\/h1>.*/\1/p')"
shot page

expect_exit 0 "the app stops" w artifact app stop "$P"
check "the pane says the app is not running" poll 20 sh -c "[ \"\$(curl -s -X POST -H 'Authorization: Bearer $(TOKEN)' -d '{\"pane\":\"$P\",\"js\":\"location.reload(), document.body.innerText\"}' http://127.0.0.1:$DEBUG_PORT/web/eval | grep -c 'app is not running')\" = 1 ]"
shot stopped
finish
