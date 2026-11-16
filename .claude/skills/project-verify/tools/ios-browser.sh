#!/usr/bin/env bash
# § iOS browser pane (wheel-zpa4.2.10.3): the phone draws a browser pane's page itself. A page on
# the Mac's loopback loads through wheeld's /loopback/<port>/, agents reach it with --on phone,
# navigation on either side moves the other, the page's console is the phone's own
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
ios_cleanup() { [ -n "$FOLLOWS" ] && kill $FOLLOWS 2>/dev/null; [ -n "$SITE_PID" ] && kill "$SITE_PID" 2>/dev/null; [ -n "$UDID" ] && "$SIM" release >/dev/null 2>&1; cleanup; }
trap ios_cleanup EXIT

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
check "wheel pane info lists the phone drawing its own page" poll 10 sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane info '$PANE' | grep -qi 'phone'"
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

# The phone moves: wheeld's page follows.
w pane eval "$PANE" 'location.href = "/two.html"; 1' --on phone >/dev/null 2>&1
check "a page the phone moves to becomes the pane's page" poll 20 page_is "http://localhost:$SPORT/two.html"
check "the phone reports where it landed" poll 10 plog "browser pane $PANE landed on http://localhost:$SPORT/two.html"

# Someone else moves the pane: the phone follows.
w pane kind "$PANE" browser --page "http://localhost:$SPORT/three.html" >/dev/null
check "a page wheeld sends the pane to loads on the phone" poll 20 plog "browser pane $PANE loading wheel-loopback://localhost:$SPORT/three.html"
check "dom h1 --on phone returns the new heading" poll 20 dom_is h1 '<h1>Unicycle</h1>'

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
w pane kind "$PANE" browser --page "http://localhost:$SPORT/" >/dev/null
check "a page load on the phone clears the phone's list" poll 20 console_lacks "from phone" --on phone
kill $FOLLOWS 2>/dev/null; FOLLOWS=""

# The app suspended: page commands say so instead of waiting.
xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1
check "with the app gone, --on phone fails with 'the phone is not attached'" poll 20 sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane dom '$PANE' h1 --on phone 2>&1 | grep -q 'the phone is not attached'"

[ "$FAIL" -eq 0 ] || { echo "--- phone.log"; grep -E "browser pane|connected|refused" "$PLOG" | tail -40; echo "--- wheeld.log"; grep -E "loopback|inspect|browser" "$(LOG)" | tail -20 | cut -c1-300; }
finish
