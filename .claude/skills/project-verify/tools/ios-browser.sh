#!/usr/bin/env bash
# § iOS browser pane (wheel-zpa4.2.10.3): the phone draws a browser pane's page itself. A page on
# the Mac's loopback loads through wheeld's /loopback/<port>/, agents reach it with --on phone,
# the phone and the Mac browse independently, a change of the pane's home (`pane home`) moves neither
# and a relaunched phone opens at it (wheel-zpa4.2.10.13), the page's console is the phone's own
# (wheel-zpa4.2.10.5: `pane console --on phone`, apart from the Mac's) and keeps what it wrote while its
# connection was down (wheel-zpa4.2.10.10; a TCP relay cuts the phone off), and a suspended app is
# "the phone is not attached".
#
# Needs WHEEL_IOS_APP: the built Wheel.app for the simulator (WheelPhone scheme), and a pool
# simulator. WHEEL_IOS_HOST (default: the Mac's first non-loopback IPv4 address) is the address the
# phone dials. WHEEL_SHOTS, when set, receives ios-browser-{phone-page,agent-banner}-{light,dark}.png.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SIM=~/.claude/tools/sim
BUNDLE=com.piercemakes.wheel.phone
HOST="${WHEEL_IOS_HOST:-$(ifconfig | awk '/inet / && $2 != "127.0.0.1" { print $2; exit }')}"
UDID=""
SITE_PID=""
FOLLOWS=""
MAC_PID=""
PX_PID=""
ios_cleanup() { [ -n "$PX_PID" ] && kill "$PX_PID" 2>/dev/null; [ -n "$MAC_PID" ] && kill "$MAC_PID" 2>/dev/null; [ -n "$FOLLOWS" ] && kill $FOLLOWS 2>/dev/null; [ -n "$SITE_PID" ] && kill "$SITE_PID" 2>/dev/null; [ -n "$UDID" ] && "$SIM" release >/dev/null 2>&1; cleanup; }
trap ios_cleanup EXIT

command -v node >/dev/null || { echo "this check plays the Mac's browser with node" >&2; exit 2; }
[ -d "${WHEEL_IOS_APP:-}" ] || { echo "set WHEEL_IOS_APP to the built Wheel.app (WheelPhone, iphonesimulator)" >&2; exit 2; }
[ -n "$HOST" ] || { echo "no non-loopback address; set WHEEL_IOS_HOST" >&2; exit 2; }

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
SPORT="$(PORT=$((RPORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
PXPORT="$(PORT=$((SPORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
CPORT="$(PORT=$((PXPORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
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
check "the phone shows which agent ran the command" poll 10 plog "browser pane $PANE .* ran dom h1 on this page"
[ -n "${WHEEL_SHOTS:-}" ] && { mkdir -p "$WHEEL_SHOTS"; for look in light dark; do
  xcrun simctl ui "$UDID" appearance "$look"; sleep 1
  w pane dom "$PANE" h1 --on phone >/dev/null 2>&1; sleep 1
  xcrun simctl io "$UDID" screenshot "$WHEEL_SHOTS/ios-browser-agent-banner-$look.png" >/dev/null 2>&1
  sleep 5
  xcrun simctl io "$UDID" screenshot "$WHEEL_SHOTS/ios-browser-phone-page-$look.png" >/dev/null 2>&1
done; xcrun simctl ui "$UDID" appearance light; }

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
check "the phone's agent banner did not name where" sh -c "! grep -q 'ran where' '$PLOG'"

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

# The app suspended: page commands say so instead of waiting.
xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1
check "with the app gone, --on phone fails with 'the phone is not attached'" poll 20 sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane dom '$PANE' h1 --on phone 2>&1 | grep -q 'the phone is not attached'"
check "with the app gone, pane where --on phone fails with 'the phone is not attached'" sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane where '$PANE' --on phone 2>&1 | grep -q 'the phone is not attached'"

[ "$FAIL" -eq 0 ] || { echo "--- phone.log"; grep -E "browser pane|connected|refused" "$PLOG" | tail -40; echo "--- wheeld.log"; grep -E "loopback|inspect|browser" "$(LOG)" | tail -20 | cut -c1-300; }
finish
