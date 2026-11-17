#!/usr/bin/env bash
# § iOS browser pane (wheel-zpa4.2.10.3): the phone draws a browser pane's page itself. A page on
# the Mac's loopback loads through wheeld's /loopback/<port>/, agents reach it with --on phone,
# the phone and the Mac browse independently, a change of the pane's home (`pane home`) moves neither
# and a relaunched phone opens at it (wheel-zpa4.2.10.13), the page's console is the phone's own
# (wheel-zpa4.2.10.5: `pane console --on phone`, apart from the Mac's), and a suspended app is
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
ios_cleanup() { [ -n "$MAC_PID" ] && kill "$MAC_PID" 2>/dev/null; [ -n "$FOLLOWS" ] && kill $FOLLOWS 2>/dev/null; [ -n "$SITE_PID" ] && kill "$SITE_PID" 2>/dev/null; [ -n "$UDID" ] && "$SIM" release >/dev/null 2>&1; cleanup; }
trap ios_cleanup EXIT

command -v node >/dev/null || { echo "this check plays the Mac's browser with node" >&2; exit 2; }
[ -d "${WHEEL_IOS_APP:-}" ] || { echo "set WHEEL_IOS_APP to the built Wheel.app (WheelPhone, iphonesimulator)" >&2; exit 2; }
[ -n "$HOST" ] || { echo "no non-loopback address; set WHEEL_IOS_HOST" >&2; exit 2; }

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
SPORT="$(PORT=$((RPORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "0.0.0.0:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

# A page on the Mac's loopback only: 127.0.0.1, which the phone cannot dial itself.
SITE="$SCR/site"
mkdir -p "$SITE/assets"
printf '<!doctype html><meta name=viewport content="width=device-width"><title>one</title><link rel=stylesheet href="/assets/s.css"><h1>Bicycle</h1><p id=styled>A bicycle has two wheels.</p>\n' >"$SITE/index.html"
printf '<!doctype html><title>two</title><h1>Tandem</h1>\n' >"$SITE/two.html"
printf '<!doctype html><title>three</title><h1>Unicycle</h1>\n' >"$SITE/three.html"
printf '#styled { color: rgb(1, 2, 3) }\n' >"$SITE/assets/s.css"
python3 -m http.server "$SPORT" --bind 127.0.0.1 --directory "$SITE" >"$SCR/site.log" 2>&1 &
SITE_PID=$!
check "the loopback page serves on 127.0.0.1:$SPORT" poll 20 curl -sf "http://127.0.0.1:$SPORT/"

# The Mac's own browser, played by node: a local connection that offers to render, holds a page
# (the pane's first page until told to go elsewhere) and answers `eval location.href` with it.
cat >"$SCR/mac.js" <<'EOF'
// usage: node mac.js <ws url> <first page>
let page = process.argv[3];
const ws = new WebSocket(process.argv[2]);
ws.onopen = () => ws.send(JSON.stringify({ op: 'render_offer' }));
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.ev !== 'inspect_pane') return;
  if (m.action === 'open') page = m.arg;
  const text = m.action === 'eval' ? JSON.stringify(page) : '';
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

code="$(w remote pair --address "$HOST")"
SIMCTL_CHILD_WHEEL_PAIR_JSON="$code" SIMCTL_CHILD_WHEEL_NOTIFICATIONS=off xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
check "the phone pairs at $HOST" poll 40 plog "connected to wss://$HOST:$RPORT/ws"
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
layout_holds_first_page() { local ls; ls="$(w pane info "$PANE" 2>&1)"; echo "$ls" | grep -q "localhost:$SPORT/" && ! echo "$ls" | grep -q "two.html"; }
check "the layout still holds the page the pane opened at" layout_holds_first_page

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

# The app suspended: page commands say so instead of waiting.
xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1
check "with the app gone, --on phone fails with 'the phone is not attached'" poll 20 sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane dom '$PANE' h1 --on phone 2>&1 | grep -q 'the phone is not attached'"

[ "$FAIL" -eq 0 ] || { echo "--- phone.log"; grep -E "browser pane|connected|refused" "$PLOG" | tail -40; echo "--- wheeld.log"; grep -E "loopback|inspect|browser" "$(LOG)" | tail -20 | cut -c1-300; }
finish
