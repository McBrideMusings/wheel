#!/usr/bin/env bash
# § iOS browser pane (wheel-zpa4.2.10.3): the phone draws a browser pane's page itself. A page on
# the Mac's loopback loads through wheeld's /loopback/<port>/, agents reach it with --on phone,
# the phone and the Mac browse independently, a change of the pane's home (`pane home`) moves neither
# and a relaunched phone opens at it (wheel-zpa4.2.10.13), the page's console is the phone's own
# (wheel-zpa4.2.10.5: `pane console --on phone`, apart from the Mac's) and keeps what it wrote while its
# connection was down (wheel-zpa4.2.10.10; a TCP relay cuts the phone off), a loopback page reaches
# another loopback port that is a browser pane's home under CORS, as the Mac's browser would, and no
# other port or site (wheel-zpa4.2.10.7), and a suspended app is "the phone is not attached".
#
# Needs WHEEL_IOS_APP: the built Wheel.app for the simulator (WheelPhone scheme), and a pool
# simulator. WHEEL_IOS_HOST (default: the Mac's first non-loopback IPv4 address) is the address the
# phone dials. WHEEL_SHOTS, when set, receives ios-browser-phone-page-{light,dark}.png.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SIM=~/.claude/tools/sim
BUNDLE=com.piercemakes.wheel.phone
HOST="${WHEEL_IOS_HOST:-$(ifconfig | awk '/inet / && $2 != "127.0.0.1" { print $2; exit }')}"
UDID=""
SITE_PID=""
FOLLOWS=""
MAC_PID=""
PX_PID=""
API_PID=""
FOREIGN_PID=""
ios_cleanup() { [ -n "$API_PID" ] && kill "$API_PID" 2>/dev/null; [ -n "$FOREIGN_PID" ] && kill "$FOREIGN_PID" 2>/dev/null; [ -n "$PX_PID" ] && kill "$PX_PID" 2>/dev/null; [ -n "$MAC_PID" ] && kill "$MAC_PID" 2>/dev/null; [ -n "$FOLLOWS" ] && kill $FOLLOWS 2>/dev/null; [ -n "$SITE_PID" ] && kill "$SITE_PID" 2>/dev/null; [ -n "$UDID" ] && "$SIM" release >/dev/null 2>&1; cleanup; }
trap ios_cleanup EXIT

command -v node >/dev/null || { echo "this check plays the Mac's browser with node" >&2; exit 2; }
[ -d "${WHEEL_IOS_APP:-}" ] || { echo "set WHEEL_IOS_APP to the built Wheel.app (WheelPhone, iphonesimulator)" >&2; exit 2; }
[ -n "$HOST" ] || { echo "no non-loopback address; set WHEEL_IOS_HOST" >&2; exit 2; }

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
SPORT="$(PORT=$((RPORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
PXPORT="$(PORT=$((SPORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
CPORT="$(PORT=$((PXPORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
BPORT="$(PORT=$((CPORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
NPORT="$(PORT=$((BPORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
EPORT="$(PORT=$((NPORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "0.0.0.0:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

# A page on the Mac's loopback only: 127.0.0.1, which the phone cannot dial itself.
SITE="$SCR/site"
mkdir -p "$SITE/assets"
printf '<!doctype html><meta name=viewport content="width=device-width"><title>one</title><link rel=stylesheet href="/assets/s.css"><h1>Bicycle</h1><p id=styled>A bicycle has two wheels.</p>\n' >"$SITE/index.html"
printf '<!doctype html><title>two</title><h1>Tandem</h1>\n' >"$SITE/two.html"
printf '<!doctype html><title>three</title><h1>Unicycle</h1>\n' >"$SITE/three.html"
printf '<!doctype html><title>five</title><script>var k = 0; setInterval(function () { console.log("third " + (++k)) }, 1000)</script>\n' >"$SITE/five.html"
printf '#styled { color: rgb(1, 2, 3) }\n' >"$SITE/assets/s.css"
python3 -m http.server "$SPORT" --bind 127.0.0.1 --directory "$SITE" >"$SCR/site.log" 2>&1 &
SITE_PID=$!
check "the loopback page serves on 127.0.0.1:$SPORT" poll 20 curl -sf "http://127.0.0.1:$SPORT/"

# The Mac's own browser, played by node: a local connection that offers to render, holds a page
# (the pane's first page until told to go elsewhere) and answers `eval location.href` and `where` with it.
cat >"$SCR/mac.js" <<'EOF'
// usage: node mac.js <ws url> <first page>
let page = process.argv[3];
const ws = new WebSocket(process.argv[2]);
ws.onopen = () => ws.send(JSON.stringify({ op: 'render_offer' }));
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.ev !== 'inspect_pane') return;
  if (m.action === 'open') page = m.arg;
  const text = m.action === 'eval' ? JSON.stringify(page) : m.action === 'where' ? JSON.stringify({ url: page, title: 'mac' }) : '';
  ws.send(JSON.stringify({ op: 'inspected', id: m.id, text }));
};
ws.onerror = () => { console.error('connection refused'); process.exit(1); };
EOF
node "$SCR/mac.js" "ws://127.0.0.1:$PORT/ws?token=$(cat "$INST/token")" "http://localhost:$SPORT/" >"$SCR/mac.log" 2>&1 &
MAC_PID=$!

PROJ="$SCR/proj"
mkdir -p "$PROJ"
w space new "$PROJ" >/dev/null
PANE="$(w space ls --json | jget 0.panes.0)"
w pane kind "$PANE" browser --page "http://localhost:$SPORT/" >/dev/null

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
xcrun simctl install "$UDID" "$WHEEL_IOS_APP"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
PLOG="$DATA/Documents/phone.log"
rm -f "$PLOG"
plog() { grep -q -- "$1" "$PLOG" 2>/dev/null; }
page_is() { w pane info "$PANE" 2>/dev/null | grep -q -- "$1"; }
dom_is() { [ "$(w pane dom "$PANE" "$1" --on phone 2>&1)" = "$2" ]; }

# The phone dials wheeld through a TCP relay that this script can cut and restore: a stopped
# wheeld keeps its sockets open, so the phone would only stall; a cut relay resets them, which
# the phone sees as a lost connection and reconnects from.
cat >"$SCR/relay.js" <<'EOF'
// usage: node relay.js <listen port> <wheeld port> <control port>
// GET /down cuts every connection and refuses new ones (what was held is lost); GET /hold keeps what the
// phone sends from reaching wheeld (the phone's sends succeed, wheeld sees nothing); GET /up undoes both
// and delivers what was held.
const net = require('net'), http = require('http');
const [lp, tp, cp] = process.argv.slice(2).map(Number);
let down = false, held = false; const socks = new Set(), pend = [];
net.createServer((c) => {
  if (down) return c.destroy();
  const u = net.connect(tp, '127.0.0.1');
  for (const s of [c, u]) { socks.add(s); s.on('close', () => { socks.delete(s); c.destroy(); u.destroy(); }); s.on('error', () => {}); }
  c.on('data', (d) => held ? pend.push([u, d]) : u.write(d));
  u.pipe(c);
}).listen(lp, '0.0.0.0');
http.createServer((q, r) => {
  if (q.url === '/down') { down = true; held = false; pend.length = 0; for (const s of socks) s.destroy(); }
  if (q.url === '/hold') held = true;
  if (q.url === '/up') { down = false; held = false; for (const [u, d] of pend.splice(0)) u.write(d); }
  r.end('ok');
}).listen(cp, '127.0.0.1');
EOF
node "$SCR/relay.js" "$PXPORT" "$RPORT" "$CPORT" >"$SCR/relay.log" 2>&1 &
PX_PID=$!
relay() { curl -sf "http://127.0.0.1:$CPORT/$1" >/dev/null; }
check "the relay to wheeld's remote port is up" poll 20 sh -c "(exec 3<>/dev/tcp/127.0.0.1/$PXPORT)"
code="$(w remote pair --address "$HOST" | sed "s/:$RPORT\//:$PXPORT\//")"
SIMCTL_CHILD_WHEEL_PAIR_JSON="$code" SIMCTL_CHILD_WHEEL_NOTIFICATIONS=off xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
check "the phone pairs at $HOST" poll 40 plog "connected to wss://$HOST:$PXPORT/ws"
check "the phone loads the pane's loopback page under its own scheme" poll 30 plog "browser pane $PANE loading wheel-loopback://localhost:$SPORT/"
check "the page arrives through wheeld's /loopback" poll 30 plog "browser pane $PANE loaded"
check "wheeld logs the forwarded request with the device and port" poll 10 grep -q "loopback forwarded.*port=$SPORT" "$(LOG)"
check "dom h1 --on phone returns the phone's heading" poll 20 dom_is h1 '<h1>Bicycle</h1>'
check "the page's root-relative stylesheet loaded (the styled color is applied)" \
  sh -c "[ \"\$('$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane eval '$PANE' 'getComputedStyle(document.getElementById(\"styled\")).color' --on phone)\" = '\"rgb(1, 2, 3)\"' ]"
check "the phone logs no agent banner after dom h1" sh -c "! grep -q ' on this page' '$PLOG'"
[ -n "${WHEEL_SHOTS:-}" ] && { mkdir -p "$WHEEL_SHOTS"; for look in light dark; do
  xcrun simctl ui "$UDID" appearance "$look"; sleep 1
  w pane dom "$PANE" h1 --on phone >/dev/null 2>&1; sleep 1
  xcrun simctl io "$UDID" screenshot "$WHEEL_SHOTS/ios-browser-phone-page-$look.png" >/dev/null 2>&1
done; xcrun simctl ui "$UDID" appearance light; }

# Another loopback port (wheel-zpa4.2.10.7): an API on 127.0.0.1:$BPORT, a second browser pane's
# home, answers the page on $SPORT as a program answers the Mac's browser, under CORS with the
# Mac's origin http://localhost:$SPORT. The same API on $NPORT is no pane's home. Every request it
# gets is logged with its Origin, the preflight's Access-Control-Request-Headers and its cookies.
cat >"$SCR/api.js" <<'EOF'
// usage: node api.js <log> <allowed origin> <port>...
const http = require('http'), fs = require('fs');
const [log, allow, ...ports] = process.argv.slice(2);
const gif = Buffer.from('R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7', 'base64');
const serve = (port) => (q, r) => {
  fs.appendFileSync(log, `${port} ${q.method} ${q.url} origin=${q.headers.origin || 'none'} acrh=${q.headers['access-control-request-headers'] || '-'} cookie=${q.headers.cookie || 'none'} host=${q.headers.host} inm=${q.headers['if-none-match'] || '-'}\n`);
  const send = (status, headers, body) => { r.writeHead(status, Object.assign({ 'Content-Type': 'text/plain' }, headers)); r.end(body); };
  const acao = { 'Access-Control-Allow-Origin': allow };
  switch (q.url) {
    case '/allowed': return send(200, acao, 'from B');
    case '/bare': return send(200, {}, 'bare');
    case '/pre':
      if (q.method === 'OPTIONS') return send(204, { ...acao, 'Access-Control-Allow-Headers': 'X-Wheel-Test', 'Access-Control-Allow-Methods': 'PUT' }, '');
      return send(200, acao, `preflighted ${q.method} ${q.headers['x-wheel-test']}`);
    case '/nopre':
      if (q.method === 'OPTIONS') return send(404, {}, 'no');
      return send(200, acao, 'should not arrive');
    case '/cred': return send(200, { ...acao, 'Access-Control-Allow-Credentials': 'true' }, `cookie=${(q.headers.cookie || 'none').split('; ').sort().join(',')}`);
    case '/setc': return send(200, { ...acao, 'Set-Cookie': 'fromb=1; Path=/' }, 'set');
    case '/setc-cred': return send(200, { ...acao, 'Access-Control-Allow-Credentials': 'true', 'Set-Cookie': 'fromb2=1; Path=/' }, 'set');
    case '/set-none': return send(200, { 'Set-Cookie': 'ss_none=1; Path=/; SameSite=None' }, 'set');
    case '/set-lax': return send(200, { 'Set-Cookie': 'ss_lax=1; Path=/; SameSite=Lax' }, 'set');
    case '/set-strict': return send(200, { 'Set-Cookie': 'ss_strict=1; Path=/; SameSite=Strict' }, 'set');
    case '/redir-ok': return send(302, { ...acao, Location: '/allowed' }, '');
    case '/redir-bare': return send(302, { Location: '/allowed' }, '');
    case '/redir-pre':
      if (q.method === 'OPTIONS') return send(204, { ...acao, 'Access-Control-Allow-Headers': 'X-Wheel-Test' }, '');
      return send(307, { ...acao, Location: '/pre' }, '');
    case '/cors.js': {
      const cached = { ...acao, ETag: '"c1"', 'Cache-Control': 'max-age=0', 'Content-Type': 'text/javascript' };
      if (q.headers['if-none-match'] === '"c1"') return send(304, cached, '');
      return send(200, cached, 'window.corsRan = (window.corsRan || 0) + 1');
    }
    case '/img': return send(200, { 'Content-Type': 'image/gif' }, gif);
    default: return send(200, { 'Content-Type': 'text/html' }, '<!doctype html><title>api</title><h1>API</h1>');
  }
};
for (const p of ports) http.createServer(serve(p)).listen(Number(p), '127.0.0.1');
EOF
node "$SCR/api.js" "$SCR/api.log" "http://localhost:$SPORT" "$BPORT" "$NPORT" >"$SCR/api.out" 2>&1 &
API_PID=$!
check "the API serves on 127.0.0.1:$BPORT and :$NPORT" poll 20 sh -c "curl -sf http://127.0.0.1:$BPORT/ >/dev/null && curl -sf http://127.0.0.1:$NPORT/ >/dev/null"
P2="$(w pane split "$PANE" right)"
w pane kind "$P2" browser --page "http://localhost:$BPORT/" >/dev/null
check "the phone draws the API pane's page" poll 30 plog "browser pane $P2 loaded"
: >"$SCR/api.log"
# `xfetch <name> <js promise of text>` starts a request on the phone's page; `xgot <name>` prints
# how it ended: `ok <text>` or `failed <error>`. The page names the API as an app does, from its own
# location (`//localhost:<port>`), so the request stays under the page's scheme.
xfetch() { w pane eval "${XP:-$PANE}" "window.x_$1 = 'pending'; Promise.resolve().then(function () { return $2 }).then(function (t) { window.x_$1 = 'ok ' + t }, function (e) { window.x_$1 = 'failed ' + e }); 1" --on phone >/dev/null 2>&1; }
xgot() { w pane eval "${XP:-$PANE}" "window.x_$1" --on phone 2>&1; }
xis() { [ "$(xgot "$1")" = "\"$2\"" ]; }
xfailed() { xgot "$1" | grep -q '^"failed '; }
api_has() { grep -q -- "$1" "$SCR/api.log"; }
api_lacks() { ! grep -q -- "$1" "$SCR/api.log"; }
B="'//' + location.hostname + ':$BPORT"
text() { echo "fetch($B$1'${2:+, $2}).then(function (r) { return r.text() })"; }
# (a) a simple request the API allows: its text arrives, sent with the Mac's origin.
xfetch a "$(text /allowed)"
check "(a) the page on :$SPORT fetches :$BPORT/allowed (Allow-Origin http://localhost:$SPORT) and reads its text" poll 20 xis a "ok from B"
check "(a) the API saw Origin http://localhost:$SPORT, the Mac's origin for the page" api_has "^$BPORT GET /allowed origin=http://localhost:$SPORT "
check "(a) wheeld forwarded it to :$BPORT" grep -q "loopback forwarded.*port=$BPORT" "$(LOG)"
check "(a) the API saw Host localhost:$BPORT, as the Mac's browser sends it" api_has "^$BPORT GET /allowed .* host=localhost:$BPORT "
# (a) credentials: a cookie the page set goes to the other port only when the page asks with
# credentials: 'include', and the API answers with an exact origin and Allow-Credentials.
w pane eval "$PANE" 'document.cookie = "crosskey=1; path=/"; 1' --on phone >/dev/null 2>&1
sleep 2
xfetch c1 "$(text /cred "{ credentials: 'include' }")"
check "(a) with credentials: 'include' the API's answer arrives with the page's cookie" poll 20 xis c1 "ok cookie=crosskey=1"
xfetch c2 "$(text /cred)"
check "(a) without it the request carries no cookie" poll 20 xis c2 "ok cookie=none"
xfetch c3 "$(text /allowed "{ credentials: 'include' }")"
check "(a) with credentials and no Allow-Credentials the fetch fails" poll 20 xfailed c3
# (b) the API sends no Allow-Origin: the request goes out and its answer is withheld, as on the Mac.
xfetch b "$(text /bare)"
check "(b) a fetch of :$BPORT/bare (no Allow-Origin) fails on the page" poll 20 xfailed b
check "(b) the request reached the API, which answered without CORS" api_has "^$BPORT GET /bare origin=http://localhost:$SPORT "
# (c) a custom header needs a preflight, which the handler sends because WebKit does not.
xfetch p1 "$(text /pre "{ headers: { 'X-Wheel-Test': 'yes' } }")"
check "(c) a fetch with X-Wheel-Test arrives after the API allowed it in a preflight" poll 20 xis p1 "ok preflighted GET yes"
check "(c) the preflight was an OPTIONS with the Mac's origin naming x-wheel-test" api_has "^$BPORT OPTIONS /pre origin=http://localhost:$SPORT acrh=x-wheel-test cookie=none"
xfetch p2 "$(text /pre "{ method: 'PUT', body: 'x' }")"
check "(c) a PUT the preflight allows arrives" poll 20 xis p2 "ok preflighted PUT undefined"
xfetch p3 "$(text /nopre "{ headers: { 'X-Wheel-Test': 'yes' } }")"
check "(c) the same fetch to a path whose preflight answers 404 fails" poll 20 xfailed p3
check "(c) the API saw that preflight and never the request" sh -c "grep -q '^$BPORT OPTIONS /nopre' '$SCR/api.log' && ! grep -q '^$BPORT GET /nopre' '$SCR/api.log'"
check "(c) phone.log says the preflight refused it" plog "from http://localhost:$SPORT refused by its preflight: the preflight answered 404"
# Credentials as WebKit marks them (httpShouldHandleCookies): a cross-origin request without
# credentials: 'include' keeps no cookie it is sent; with it, it does; a same-origin request with
# credentials: 'omit' sends none.
xfetch s1 "$(text /setc)"
check "(a) a cross-origin fetch without credentials of a page that sets a cookie arrives" poll 20 xis s1 "ok set"
xfetch s2 "$(text /setc-cred "{ credentials: 'include' }")"
check "(a) the same with credentials: 'include' arrives" poll 20 xis s2 "ok set"
xfetch s3 "$(text /cred "{ credentials: 'include' }")"
check "(a) only the cookie set with credentials was kept" poll 20 xis s3 "ok cookie=crosskey=1,fromb2=1"
XP=$P2 xfetch o1 "fetch('/cred', { credentials: 'omit' }).then(function (r) { return r.text() })"
check "(a) a same-origin fetch with credentials: 'omit' sends no cookie" poll 20 sh -c "[ \"\$('$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane eval '$P2' 'window.x_o1' --on phone)\" = '\"ok cookie=none\"' ]"
XP=$P2 xfetch o2 "fetch('/cred').then(function (r) { return r.text() })"
check "(a) and without it sends the page's" poll 20 sh -c "[ \"\$('$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane eval '$P2' 'window.x_o2' --on phone)\" = '\"ok cookie=crosskey=1,fromb2=1\"' ]"

# A CORS request's redirects: each passes the CORS check, and a preflighted one is preflighted
# again for where it goes.
: >"$SCR/api.log"
xfetch r1 "$(text /redir-ok)"
check "(c) a redirect that allows the origin is followed" poll 20 xis r1 "ok from B"
xfetch r2 "$(text /redir-bare)"
check "(c) a redirect without Allow-Origin fails the fetch" poll 20 xfailed r2
check "(c) and the request never went where it pointed" sh -c "[ \"\$(grep -c '^$BPORT GET /allowed' '$SCR/api.log')\" = 1 ]"
check "(c) phone.log says the redirect refused it" plog "refused at its redirect: Access-Control-Allow-Origin is missing"
xfetch r3 "$(text /redir-pre "{ headers: { 'X-Wheel-Test': 'yes' } }")"
check "(c) a preflighted request that redirects arrives" poll 20 xis r3 "ok preflighted GET yes"
check "(c) and was preflighted at each hop (OPTIONS /redir-pre, GET, OPTIONS /pre, GET)" sh -c "grep -E '^$BPORT [A-Z]+ /(redir-pre|pre) ' '$SCR/api.log' | awk '{print \$2, \$3}' | tr '\n' ' ' | grep -q '^OPTIONS /redir-pre GET /redir-pre OPTIONS /pre GET /pre '"

# WebKit's own revalidation headers (Cache-Control, If-None-Match) on a reload are no reason for a
# preflight: a crossorigin script on another port is revalidated with no OPTIONS.
printf '<!doctype html><title>reload</title><h1>Reload</h1><script crossorigin src="//localhost:%s/cors.js"></script>\n' "$BPORT" >"$SITE/reload.html"
w pane page "$PANE" "http://localhost:$SPORT/reload.html" --on phone >/dev/null
ran_is() { [ "$(w pane eval "$PANE" 'window.corsRan' --on phone 2>&1)" = "$1" ]; }
check "the page with a crossorigin script on :$BPORT ran it" poll 20 ran_is 1
: >"$SCR/api.log"
w pane eval "$PANE" 'location.reload(); 1' --on phone >/dev/null 2>&1
check "after a reload the script ran again" poll 20 ran_is 1
check "the reload revalidated it (If-None-Match) or fetched it again" poll 10 api_has "^$BPORT GET /cors.js "
grep "/cors.js" "$SCR/api.log" | cut -c1-200
check "and sent no preflight for WebKit's revalidation headers" api_lacks "^$BPORT OPTIONS /cors.js"

# SameSite: cookies set on 127.0.0.1:$BPORT, another site than localhost.
w pane page "$P2" "http://127.0.0.1:$BPORT/" --on phone >/dev/null
check "the API pane's phone browser is on 127.0.0.1:$BPORT (pane where --on phone)" poll 20 sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane where '$P2' --on phone 2>&1 | grep -q '^http://127.0.0.1:$BPORT/'"
XP=$P2 xfetch k1 "fetch('/set-none').then(function () { return fetch('/set-lax') }).then(function () { return fetch('/set-strict') }).then(function () { return fetch('/cred') }).then(function (r) { return r.text() })"
check "on 127.0.0.1 itself every SameSite cookie goes" poll 20 sh -c "[ \"\$('$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane eval '$P2' 'window.x_k1' --on phone)\" = '\"ok cookie=ss_lax=1,ss_none=1,ss_strict=1\"' ]"
check "the API saw Host 127.0.0.1:$BPORT for it" api_has "^$BPORT GET /cred .* host=127.0.0.1:$BPORT "
w pane page "$PANE" "http://localhost:$SPORT/" --on phone >/dev/null
check "the phone's page is back on localhost:$SPORT" poll 20 dom_is h1 '<h1>Bicycle</h1>'
xfetch k2 "fetch('//127.0.0.1:$BPORT/cred', { credentials: 'include' }).then(function (r) { return r.text() })"
check "from localhost, a credentialed fetch of 127.0.0.1 carries only the SameSite=None cookie" poll 20 xis k2 "ok cookie=ss_none=1"
w pane eval "$PANE" "location.href = '//127.0.0.1:$BPORT/cred'; 1" --on phone >/dev/null 2>&1
body_is() { [ "$(w pane eval "$PANE" 'document.body.innerText.trim()' --on phone 2>&1)" = "\"$1\"" ]; }
check "a navigation the localhost page starts to 127.0.0.1 carries None and Lax, not Strict" poll 20 body_is "cookie=ss_lax=1,ss_none=1"
w pane page "$PANE" "http://127.0.0.1:$BPORT/cred" --on phone >/dev/null
check "one the browser starts (pane page) carries all three" poll 20 body_is "cookie=ss_lax=1,ss_none=1,ss_strict=1"
w pane page "$PANE" "http://localhost:$SPORT/" --on phone >/dev/null
check "the phone's page is back on localhost:$SPORT" poll 20 dom_is h1 '<h1>Bicycle</h1>'

# (d) a port that is no browser pane's home stays out of reach, though something listens there.
xfetch d "fetch('//' + location.hostname + ':$NPORT/allowed').then(function (r) { return r.status })"
check "(d) a fetch of :$NPORT (no pane's home) fails on the page" poll 20 xfailed d
echo "--- (d) the page got: $(xgot d)"
check "(d) wheeld refused :$NPORT" grep -q "loopback refused: no browser pane is on this port.*port=$NPORT" "$(LOG)"
check "(d) the API on :$NPORT got nothing" api_lacks "^$NPORT "
# (e) a page from another site (the Mac's address, which the phone dials itself) cannot reach the
# Mac's loopback: not by fetch, not by an image, not by a form post.
FOREIGN="$SCR/foreign"
mkdir -p "$FOREIGN"
cat >"$FOREIGN/foreign.html" <<EOF
<!doctype html><title>foreign</title><h1>Foreign</h1>
<form method=post action="wheel-loopback://localhost:$BPORT/allowed"><input name=x value=1></form>
<script>
window.r = [];
fetch('wheel-loopback://localhost:$BPORT/allowed').then(function (r) { return r.text() }).then(function (t) { window.r.push('fetch ok ' + t) }, function (e) { window.r.push('fetch failed') });
var i = new Image(); i.onload = function () { window.r.push('img loaded') }; i.onerror = function () { window.r.push('img failed') }; i.src = 'wheel-loopback://localhost:$BPORT/img';
</script>
EOF
python3 -m http.server "$EPORT" --bind "$HOST" --directory "$FOREIGN" >"$SCR/foreign.log" 2>&1 &
FOREIGN_PID=$!
check "the foreign page serves on $HOST:$EPORT" poll 20 curl -sf "http://$HOST:$EPORT/foreign.html"
: >"$SCR/api.log"
w pane page "$P2" "http://$HOST:$EPORT/foreign.html" --on phone >/dev/null
foreign_got() { [ "$(w pane eval "$P2" 'window.r.slice().sort().join(",")' --on phone 2>&1)" = '"fetch failed,img failed"' ]; }
check "(e) on the foreign page, the fetch and the image of the loopback both fail" poll 30 foreign_got
w pane eval "$P2" 'document.forms[0].submit(); 1' --on phone >/dev/null 2>&1
check "(e) its form post to the loopback is cancelled" poll 20 plog "browser pane $P2 refused a POST to the Mac's loopback from another site"
check "(e) the image was refused by the phone's handler, naming the page's origin" plog "browser pane $P2 request refused: wheel-loopback://localhost:$BPORT/img asked by a page not on the Mac's loopback"
check "(e) the API got nothing from the foreign page" sh -c "! [ -s '$SCR/api.log' ]"
w pane kind "$P2" empty >/dev/null

# The phone moves on its own: the Mac's browser stays where it was, and the layout keeps the page
# the pane opened at.
w pane eval "$PANE" 'location.href = "/two.html"; 1' --on phone >/dev/null 2>&1
check "the phone moved to /two.html (dom h1 --on phone returns its heading)" poll 20 dom_is h1 '<h1>Tandem</h1>'
check "the Mac's browser (eval location.href) still names the first page" sh -c "[ \"\$('$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane eval '$PANE' 'location.href')\" = '\"http://localhost:$SPORT/\"' ]"
check "the layout still holds the page the pane opened at (pane home)" sh -c "[ \"\$('$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane home '$PANE')\" = 'http://localhost:$SPORT/' ]"

# wheeld holds no browser's page: it asks (wheel-zpa4.2.10.15). pane info names the home and each
# attached browser's own page; pane where prints one browser's page and title.
info_has() { w pane info "$PANE" 2>&1 | grep -q -- "$1"; }
where_is() { [ "$(w pane where "$PANE" "${@:2}" 2>&1)" = "$1" ]; }
check "pane info names the home" info_has "^Home: http://localhost:$SPORT/\$"
check "pane info names the phone's new page and title" poll 10 info_has "^Phone .*: http://localhost:$SPORT/two.html (two)"
check "pane info names the Mac's page" info_has "^Mac: http://localhost:$SPORT/ (mac)"
check "pane where --on phone prints the phone's page and title" where_is "$(printf 'http://localhost:%s/two.html\ttwo' "$SPORT")" --on phone
check "pane where prints the Mac's page and title" where_is "$(printf 'http://localhost:%s/\tmac' "$SPORT")"
check "the phone logs no agent banner after where" sh -c "! grep -q ' on this page' '$PLOG'"

# `pane page --on phone` moves the phone alone; `--on mac` moves the Mac alone.
w pane page "$PANE" "http://localhost:$SPORT/three.html" --on phone >/dev/null
check "pane page --on phone loads the page on the phone" poll 20 plog "browser pane $PANE loading wheel-loopback://localhost:$SPORT/three.html"
check "dom h1 --on phone returns the new heading" poll 20 dom_is h1 '<h1>Unicycle</h1>'
check "the Mac's browser did not move" sh -c "[ \"\$('$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane eval '$PANE' 'location.href')\" = '\"http://localhost:$SPORT/\"' ]"
w pane page "$PANE" "http://localhost:$SPORT/two.html" --on mac >/dev/null
check "pane page --on mac moves the Mac's browser" sh -c "[ \"\$('$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane eval '$PANE' 'location.href')\" = '\"http://localhost:$SPORT/two.html\"' ]"
check "and the phone is still on the third page" dom_is h1 '<h1>Unicycle</h1>'

# The phone's page writes to its console: wheeld keeps it as the phone's, apart from the Mac's.
console_has() { local want="$1"; shift; w pane console "$PANE" "$@" 2>&1 | grep -q -- "$want"; }
console_lacks() { local want="$1"; shift; ! w pane console "$PANE" "$@" 2>&1 | grep -q -- "$want"; }
w pane console "$PANE" --on phone --follow >"$SCR/follow-phone.txt" 2>&1 &
FOLLOWS="$!"
w pane console "$PANE" --follow >"$SCR/follow-mac.txt" 2>&1 &
FOLLOWS="$FOLLOWS $!"
sleep 1
w pane eval "$PANE" 'console.log("from phone"); 1' --on phone >/dev/null 2>&1
check "pane console --on phone prints what the phone's page logged" poll 20 console_has " log  *from phone" --on phone
check "pane console (the Mac's) does not" console_lacks "from phone"
check "pane console --on phone --follow printed it as it came" poll 10 grep -q " log  *from phone" "$SCR/follow-phone.txt"
check "pane console --follow (the Mac's) did not" sh -c "! grep -q 'from phone' '$SCR/follow-mac.txt'"
w pane eval "$PANE" 'setTimeout(function () { throw new Error("phone boom") }); 1' --on phone >/dev/null 2>&1
check "an uncaught error on the phone's page reaches --on phone as error" poll 20 console_has " error Uncaught Error: phone boom" --on phone
check "a script on the Mac's loopback is named as the Mac names it" console_has "(http://localhost:$SPORT/three.html:1)" --on phone
echo "--- pane console $PANE --on phone"; w pane console "$PANE" --on phone 2>&1 | tail -5
echo "--- pane console $PANE"; w pane console "$PANE" 2>&1 | tail -5
check "wheeld logs each phone message with on=phone" poll 10 grep -q 'on=phone.*text=from phone' "$(LOG)"
w pane eval "$PANE" 'location.reload(); 1' --on phone >/dev/null 2>&1
check "a page load on the phone clears the phone's list" poll 20 console_lacks "from phone" --on phone
kill $FOLLOWS 2>/dev/null; FOLLOWS=""

# The pane's home (wheel-zpa4.2.10.13): `wheel pane home` sets it and moves no browser; a browser
# made afterwards opens at it.
home_is() { [ "$(w pane home "$PANE" 2>&1)" = "$1" ]; }
check "pane home prints the page the pane opened at" home_is "http://localhost:$SPORT/"
w pane home "$PANE" "http://localhost:$SPORT/index.html" >/dev/null
check "pane home <page> sets the home" home_is "http://localhost:$SPORT/index.html"
sleep 3
check "the phone did not move when the home changed (dom h1 --on phone unchanged)" dom_is h1 '<h1>Unicycle</h1>'
check "the Mac's browser did not move when the home changed" sh -c "[ \"\$('$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane eval '$PANE' 'location.href')\" = '\"http://localhost:$SPORT/two.html\"' ]"
xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1
rm -f "$PLOG"
# The pairing code is single use: the relaunched app reconnects with the credential it saved.
SIMCTL_CHILD_WHEEL_NOTIFICATIONS=off xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
check "the relaunched phone opens at the new home" poll 40 plog "browser pane $PANE loading wheel-loopback://localhost:$SPORT/index.html"
check "dom h1 --on phone returns the home page's heading" poll 20 dom_is h1 '<h1>Bicycle</h1>'

# Lines the phone's page writes while its connection is down are kept (wheel-zpa4.2.10.10). The
# page counts once a second; the relay cuts the phone off for 5 seconds; every number is there.
ticks() { w pane console "$PANE" --on phone 2>&1 | grep -o 'tick [0-9]*' | awk '{print $2}'; }
ticks_whole() { local t; t="$(ticks)"; [ -n "$t" ] && [ "$(echo "$t" | awk 'NR==1{p=$1;next} {if ($1!=p+1) bad=1; p=$1} END{print bad+0}')" = 0 ]; }
ticks_past() { [ "$(ticks | tail -1)" -ge "$1" ] 2>/dev/null; }
connects() { grep -c 'connected to wss' "$PLOG"; }
drop_phone() { local before; before="$(connects)"; relay down; date -u +%Y-%m-%dT%H:%M:%S >"$SCR/down_at"; sleep "$1"; date -u +%Y-%m-%dT%H:%M:%S >"$SCR/up_at"; relay up; poll 60 sh -c "[ \"\$(grep -c 'connected to wss' '$PLOG')\" -gt $before ]"; }
w pane eval "$PANE" 'window.n = 0; setInterval(function () { console.log("tick " + (++window.n)) }, 1000); 1' --on phone >/dev/null 2>&1
check "the phone's page is counting in pane console --on phone" poll 20 ticks_past 3
check "the phone's connection dropped for 5 seconds and came back" drop_phone 5
check "the page's count went on after the reconnect" poll 30 ticks_past "$(( $(ticks | tail -1) + 3 ))"
check "pane console --on phone has every number, none missing across the outage" ticks_whole
echo "--- ticks: $(ticks | tr '\n' ' ')"

# Lines sent just before the cut, whose ok never comes back, are sent again on the next connection:
# the relay holds what the phone sends for 4 seconds (wheeld hears nothing), then cuts it.
relay hold
sleep 4
check "the phone's connection dropped with 4 seconds of lines unanswered" drop_phone 2
check "the page's count went on after the second reconnect" poll 30 ticks_past "$(( $(ticks | tail -1) + 3 ))"
check "pane console --on phone still has every number, none missing and none twice" ticks_whole
echo "--- ticks: $(ticks | tr '\n' ' ') (twice: $(ticks | sort -n | uniq -d | wc -l | tr -d ' '))"

# A page load while the phone is cut off clears the list, though wheeld hears of it late. A blank
# page needs no network, so the load happens during the outage.
w pane eval "$PANE" 'setTimeout(function () { location.href = "about:blank" }, 2000); 1' --on phone >/dev/null 2>&1
check "the phone's connection dropped for the page load" drop_phone 5
load_in_outage() { awk -v from="$(cat "$SCR/down_at")" -v to="$(cat "$SCR/up_at")" -v pane="$PANE" '$0 ~ "browser pane " pane " loaded$" { t = substr($1, 1, 19); if (t >= from && t <= to) found = 1 } END { exit !found }' "$PLOG"; }
check "the blank page loaded while the relay was down (phone.log, between the cut and the restore)" load_in_outage
w pane eval "$PANE" 'console.log("after load"); 1' --on phone >/dev/null 2>&1
check "the phone's list holds the lines written after the load" poll 30 console_has " after load" --on phone
only_after_load() { local out; out="$(w pane console "$PANE" --on phone 2>&1)"; [ -n "$out" ] && ! echo "$out" | grep -q 'tick'; }
check "and none from before it (the list is not empty and has no tick)" only_after_load
echo "--- pane console $PANE --on phone"; w pane console "$PANE" --on phone 2>&1 | tail -4

# A report wheeld refuses is logged on the phone, naming the pane: a second browser pane stops being one
# while the relay holds what the phone sent for it: wheeld gets the reports after the pane changed.
P3="$(w pane split "$PANE" right)"
w pane kind "$P3" browser --page "http://localhost:$SPORT/five.html" >/dev/null
check "the phone draws the second browser pane's page" poll 30 plog "browser pane $P3 loaded"
relay hold
sleep 3
w pane kind "$P3" empty >/dev/null
before="$(connects)"; relay up
check "phone.log names the pane and wheeld's refusal" poll 20 plog "browser pane $P3 console report refused: $P3 is not a browser pane"
grep "console report refused" "$PLOG" | head -2

# The bottom toolbar (wheel-zpa4.2.10.18): back, Home, Set as home and reload are the system's own buttons.
# The app's debug route presses one as its tap does (it writes "<pane> <button>" to Documents/toolbar-press)
# and logs each button's state, "on" or "off" (disabled).
printf '<!doctype html><title>stamp</title><script>window.stamp = Math.random()</script>\n' >"$SITE/stamp.html"
tb_lines() { grep -c "toolbar $PANE: $1" "$PLOG"; }
tb_press() { local n; n="$(tb_lines "press $1")"; printf '%s %s' "$PANE" "$1" >"$DATA/Documents/toolbar-press"; poll 10 sh -c "[ \"\$(grep -c 'toolbar $PANE: press $1' '$PLOG')\" -gt $n ]"; }
tb_state() { local n; n="$(tb_lines back=)"; printf '%s state' "$PANE" >"$DATA/Documents/toolbar-press"; poll 10 sh -c "[ \"\$(grep -c 'toolbar $PANE: back=' '$PLOG')\" -gt $n ]" && grep "toolbar $PANE: back=" "$PLOG" | tail -1; }
tb_is() { tb_state | grep -q -- "$1"; }
w pane page "$PANE" "http://localhost:$SPORT/index.html" --on phone >/dev/null
check "the toolbar's page is the home: dom h1 --on phone returns Bicycle" poll 20 dom_is h1 '<h1>Bicycle</h1>'
tb_images() { local s; s="$(tb_state)"; [ -n "$s" ] && ! printf %s "$s" | grep -q noimage; }
check "every toolbar button has its SF Symbol (the state line has no noimage)" poll 10 tb_images
check "(3) on the home page Set as home is disabled and the other buttons keep their slots" poll 10 tb_is "set-home=off/image"
w pane page "$PANE" "http://localhost:$SPORT/two.html" --on phone >/dev/null
check "the phone moved off the home: dom h1 --on phone returns Tandem" poll 20 dom_is h1 '<h1>Tandem</h1>'
check "back is enabled after the phone moved on" poll 10 tb_is "back=on/"
check "(3) off the home Set as home is enabled" poll 10 tb_is "set-home=on/"
check "(2) the Home button is pressed" tb_press home
check "(2) pane where --on phone names the home after Home" poll 20 sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane where '$PANE' --on phone 2>&1 | grep -q 'http://localhost:$SPORT/index.html'"
w pane page "$PANE" "http://localhost:$SPORT/three.html" --on phone >/dev/null
check "the phone moved to /three.html: dom h1 --on phone returns Unicycle" poll 20 dom_is h1 '<h1>Unicycle</h1>'
check "(3) the Set as home button is pressed" tb_press set-home
check "(3) pane home prints the page in view" poll 20 home_is "http://localhost:$SPORT/three.html"
check "(3) on the new home Set as home is disabled" poll 10 tb_is "set-home=off/image"
w pane page "$PANE" "http://localhost:$SPORT/stamp.html" --on phone >/dev/null
stamp() { w pane eval "$PANE" 'window.stamp' --on phone 2>&1; }
check "the stamp page loaded" poll 20 sh -c "[ \"\$('$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane eval '$PANE' 'typeof window.stamp' --on phone 2>&1)\" = '\"number\"' ]"
S1="$(stamp)"
check "(4) the reload button is pressed" tb_press reload
stamp_changed() { local now; now="$(stamp)"; [ "$now" != "$S1" ] && [ "$now" != '"undefined"' ] && [ -n "$now" ]; }
check "(4) the value the page writes on each load changed" poll 20 stamp_changed

# The app suspended: page commands say so instead of waiting.
xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1
check "with the app gone, --on phone fails with 'the phone is not attached'" poll 20 sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane dom '$PANE' h1 --on phone 2>&1 | grep -q 'the phone is not attached'"
check "with the app gone, pane where --on phone fails with 'the phone is not attached'" sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane where '$PANE' --on phone 2>&1 | grep -q 'the phone is not attached'"

[ "$FAIL" -eq 0 ] || { echo "--- phone.log"; grep -E "browser pane|connected|refused" "$PLOG" | tail -40; echo "--- wheeld.log"; grep -E "loopback|inspect|browser" "$(LOG)" | tail -20 | cut -c1-300; }
finish
