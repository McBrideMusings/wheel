#!/usr/bin/env bash
# § iOS browser pane, shared session (wheel-zpa4.2.10.4): before the phone's page first lands on a
# site it takes the Mac's session of it (cookies, localStorage) through wheeld; a cookie the site
# sets on the phone stays; switching the pane to Mac hands the phone's session back; and
# `wheel pane share-session <pane> off` stops all of it.
#
# The Mac is played by node: a local connection that sends render_offer, answers session_request
# with a login cookie, a page cookie and a localStorage item, and answers session_install.
#
# Needs WHEEL_IOS_APP: the built Wheel.app for the simulator (WheelPhone scheme), and a pool
# simulator. WHEEL_IOS_HOST (default: the Mac's first non-loopback IPv4 address) is the address the
# phone dials.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
command -v node >/dev/null || { echo "this check plays the Mac with node" >&2; exit 2; }
SIM=~/.claude/tools/sim
BUNDLE=com.piercemakes.wheel.phone
HOST="${WHEEL_IOS_HOST:-$(ifconfig | awk '/inet / && $2 != "127.0.0.1" { print $2; exit }')}"
UDID=""
SITE_PID=""
MAC_PID=""
ios_cleanup() {
  [ -n "$SITE_PID" ] && kill "$SITE_PID" 2>/dev/null
  [ -n "$MAC_PID" ] && kill "$MAC_PID" 2>/dev/null
  [ -n "$UDID" ] && "$SIM" release >/dev/null 2>&1
  cleanup
}
trap ios_cleanup EXIT

[ -d "${WHEEL_IOS_APP:-}" ] || { echo "set WHEEL_IOS_APP to the built Wheel.app (WheelPhone, iphonesimulator)" >&2; exit 2; }
[ -n "$HOST" ] || { echo "no non-loopback address; set WHEEL_IOS_HOST" >&2; exit 2; }

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
SPORT="$(PORT=$((RPORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "0.0.0.0:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

# A site on the Mac's loopback that logs the Cookie header of every request, and sets a cookie at /login.
cat >"$SCR/site.py" <<'EOF'
import http.server, sys
class Site(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        print(f"GET {self.path} cookie={self.headers.get('Cookie') or '-'}", flush=True)
        if self.path == "/sso":
            self.send_response(302)
            self.send_header("Location", "/landed")
            self.send_header("Set-Cookie", "redir=yes; Path=/")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        body = f"<!doctype html><title>{self.path}</title><h1>{self.path}</h1>".encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        if self.path == "/login":
            self.send_header("Set-Cookie", "fresh=phone; Path=/; Max-Age=3600")
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a):
        pass
http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Site).serve_forever()
EOF
python3 "$SCR/site.py" "$SPORT" >"$SCR/site.log" 2>&1 &
SITE_PID=$!
check "the loopback site serves on 127.0.0.1:$SPORT" poll 20 curl -sf "http://127.0.0.1:$SPORT/"

# The Mac: one JSON line per session_request and session_install it gets, in mac.log.
cat >"$SCR/mac.js" <<'EOF'
// usage: node mac.js <ws url>
const ws = new WebSocket(process.argv[2]);
ws.onopen = () => ws.send(JSON.stringify({ op: 'render_offer' }));
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.ev === 'session_request') {
    console.log(JSON.stringify(m));
    const session = {
      origin: m.origin,
      cookies: [
        { name: 'sid', value: 'mac-login', domain: 'localhost', http_only: true },
        { name: 'seen', value: 'mac', domain: 'localhost' },
      ],
      local: { theme: 'dark' },
    };
    ws.send(JSON.stringify({ op: 'session_answer', id: m.id, session }));
  } else if (m.ev === 'session_install') {
    console.log(JSON.stringify(m));
    ws.send(JSON.stringify({ op: 'session_answer', id: m.id }));
  }
};
ws.onerror = () => { console.error('connection refused'); process.exit(1); };
EOF
node "$SCR/mac.js" "ws://127.0.0.1:$PORT/ws?token=$(cat "$INST/token")" >"$SCR/mac.log" 2>&1 &
MAC_PID=$!

PROJ="$SCR/proj"
mkdir -p "$PROJ"
w space new "$PROJ" >/dev/null
PANE="$(w space ls --json | jget 0.panes.0)"
w pane kind "$PANE" browser --page "http://localhost:$SPORT/" >/dev/null
ORIGIN="http://localhost:$SPORT"

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
xcrun simctl install "$UDID" "$WHEEL_IOS_APP"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
PLOG="$DATA/Documents/phone.log"
rm -f "$PLOG"
plog() { grep -q -- "$1" "$PLOG" 2>/dev/null; }
asks() { grep -c '"ev":"session_request"' "$SCR/mac.log"; }
eval_is() { [ "$(w pane eval "$PANE" "$1" --on phone 2>&1)" = "$2" ]; }
launch() { xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1; "$@" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null; }

code="$(w remote pair --address "$HOST")"
launch env SIMCTL_CHILD_WHEEL_PAIR_JSON="$code"
check "the phone pairs at $HOST" poll 40 plog "connected to wss://$HOST:$RPORT/ws"

# Shared by default: the Mac is asked once, and the phone's first request carries the Mac's login.
check "wheeld asks the Mac for its session of $ORIGIN" poll 30 grep -q "\"origin\":\"$ORIGIN\"" "$SCR/mac.log"
check "the phone takes it: 2 cookies, 1 stored item" poll 20 plog "browser pane $PANE took the Mac's session of $ORIGIN: 2 cookies, 1 stored items"
check "the page's first request carries the Mac's HttpOnly login cookie" poll 20 grep -q "GET / cookie=.*sid=mac-login" "$SCR/site.log"
check "the page reads the Mac's localStorage" poll 20 eval_is 'localStorage.getItem("theme")' '"dark"'
check "document.cookie on the loopback page shows the Mac's non-HttpOnly cookie" poll 10 sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane eval '$PANE' 'document.cookie' --on phone | grep -q 'seen=mac'"
check "document.cookie hides the HttpOnly one" sh -c "! '$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane eval '$PANE' 'document.cookie' --on phone | grep -q 'sid='"
check "wheel pane info says the pane shares its session" sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane info '$PANE' | grep -q 'Shares session: on (browser.share_session)'"

w pane eval "$PANE" 'document.cookie = "js=page; Path=/"; document.cookie = "sid=forged; Path=/"; 1' --on phone >/dev/null 2>&1
check "a cookie the page's script sets reads back" poll 10 sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane eval '$PANE' 'document.cookie' --on phone | grep -q 'js=page'"

# A cookie the site sets on the phone (a custom scheme, which WebKit keeps none for) stays.
w pane eval "$PANE" 'location.href = "/login"; 1' --on phone >/dev/null 2>&1
check "the next request carries the script's cookie" poll 20 grep -q "GET /login cookie=.*js=page" "$SCR/site.log"
check "the script could not overwrite the HttpOnly login" sh -c "grep 'GET /login ' '$SCR/site.log' | grep -q 'sid=mac-login' && ! grep -q 'sid=forged' '$SCR/site.log'"
check "the phone opens /login" poll 20 grep -q "GET /login " "$SCR/site.log"
w pane eval "$PANE" 'location.href = "/after"; 1' --on phone >/dev/null 2>&1
check "the next request carries the cookie /login set" poll 20 grep -q "GET /after cookie=.*fresh=phone" "$SCR/site.log"
w pane eval "$PANE" 'location.href = "/sso"; 1' --on phone >/dev/null 2>&1
check "a cookie set on a 302 reaches the page it redirects to" poll 20 grep -q "GET /landed cookie=.*redir=yes" "$SCR/site.log"
check "the /after page's script sees the cookie /login set" poll 20 sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane eval '$PANE' 'document.cookie' --on phone | grep -q 'fresh=phone'"
eq "the Mac was asked once for the origin" "$(asks)" 1

# The page logs a cookie out: the session the Mac gets lacks it, and the Mac deletes its own.
w pane eval "$PANE" 'document.cookie = "seen=; Path=/; Max-Age=0"; 1' --on phone >/dev/null 2>&1
check "a cookie the page deletes is gone from its document.cookie" poll 10 sh -c "! '$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane eval '$PANE' 'document.cookie' --on phone | grep -q 'seen='"

# The pane goes back to the Mac: the phone hands its session back, the new cookie included.
launch env SIMCTL_CHILD_WHEEL_MENU=mode-mac
check "the phone hands its session back" poll 30 plog "browser pane $PANE hands back the session of $ORIGIN"
check "the Mac is asked to install it, with the cookie the phone got" poll 20 sh -c "grep '\"ev\":\"session_install\"' '$SCR/mac.log' | grep -q '\"name\":\"fresh\",\"value\":\"phone\"'"
check "the session handed back lacks the cookie the page deleted" sh -c "! grep '\"ev\":\"session_install\"' '$SCR/mac.log' | grep -q '\"name\":\"seen\"'"
eq "a relaunch while the phone draws the pane takes no session again" "$(asks)" 1

# Sharing off on the pane: the phone draws it again and wheeld refuses; the Mac never hears of it.
w pane share-session "$PANE" off >/dev/null
BEFORE="$(asks)"
check "wheel pane info says sharing is off on this pane" sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane info '$PANE' | grep -q 'Shares session: off (set on this pane)'"
launch env SIMCTL_CHILD_WHEEL_MENU=mode-phone
check "the phone draws the page again" poll 30 plog "browser pane $PANE drawn by the phone"
check "wheeld refuses the phone's ask, naming the pane's switch" poll 30 plog "browser pane $PANE has no session of $ORIGIN from the Mac: pane $PANE does not share its session (off: set on this pane)"
eq "the Mac was not asked again" "$(asks)" "$BEFORE"

# Back to following the setting, which the project turns off.
w pane share-session "$PANE" follow >/dev/null
printf '[browser]\nshare_session = false\n' >"$PROJ/wheel.toml"
check "wheel pane info names the setting" sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane info '$PANE' | grep -q 'Shares session: off (browser.share_session)'"
rm -f "$PLOG"
launch env
check "wheeld refuses the phone's ask, naming the setting" poll 30 plog "browser pane $PANE has no session of $ORIGIN from the Mac: pane $PANE does not share its session (off: browser.share_session)"
eq "the Mac was still not asked" "$(asks)" "$BEFORE"

[ "$FAIL" -eq 0 ] || { echo "--- phone.log"; grep -E "browser pane|connected|menu" "$PLOG" | tail -40; echo "--- mac.log"; cut -c1-300 "$SCR/mac.log"; echo "--- site.log"; cat "$SCR/site.log"; }
finish
