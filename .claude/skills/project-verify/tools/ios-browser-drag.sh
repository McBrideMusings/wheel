#!/usr/bin/env bash
# Phone browser drags (wheel-zpa4.2.10.21): on a browser pane, left and right drags reach the page
# and never change pane or go back; the top bar and an agent page still change pane. Driven with
# real drags by WheelPhoneUITests/BrowserDragUITests against this script's own wheeld. At each step
# the test writes `$SYNC/step-<name>`; this script checks wheeld, the page and phone.log, then
# acknowledges.
#
# Needs WHEEL_IOS_DERIVED_DATA: a build-for-testing of the WheelPhone scheme for the simulator (see
# project-verify § iOS title menu). Run with `env -u WHEEL_SPACE -u WHEEL_PANE`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/ios-ui-lib.sh"

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
SPORT="$(PORT=$((RPORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

# A page that records how far a finger travels left and right, on the Mac's loopback.
SITE="$SCR/site"
mkdir -p "$SITE"
cat >"$SITE/index.html" <<'EOF'
<!doctype html><meta name=viewport content="width=device-width,initial-scale=1"><title>board</title>
<style>html,body{margin:0;height:100%;touch-action:none}</style><h1>Board</h1>
<script>
window.left = 0; window.right = 0; var x0 = null;
addEventListener('pointerdown', function (e) { x0 = e.clientX });
addEventListener('pointermove', function (e) { if (x0 === null) return; var d = e.clientX - x0; if (d < 0) left = Math.max(left, -d); else right = Math.max(right, d) });
addEventListener('pointerup', function () { x0 = null });
</script>
EOF
cp "$SITE/index.html" "$SITE/two.html"
sed -i '' 's#<h1>Board</h1>#<h1>Board two</h1>#' "$SITE/two.html"
python3 -m http.server "$SPORT" --bind 127.0.0.1 --directory "$SITE" >"$SCR/site.log" 2>&1 &
SITE_PID=$!
trap 'kill "$SITE_PID" 2>/dev/null; ios_cleanup' EXIT
check "the page serves on 127.0.0.1:$SPORT" poll 20 curl -sf "http://127.0.0.1:$SPORT/"

mkdir -p "$SCR/proj"
S="$(w space new "$SCR/proj")"
P1="$(w space ls | jq -r --arg s "$S" '[.spaces[] | select(.id==$s) | .root | .. | select(.node? == "pane")][0].id')"
P2="$(w pane split "$P1" right)"
w pane kind "$P1" browser --page "http://localhost:$SPORT/index.html" >/dev/null
w pane start "$P2" agent >/dev/null
w pane focus "$P1" >/dev/null

phone_pane() { w client ls --json | jq -r '.[].windows[] | select(.window=="phone") | .pane // "none"'; }
expect_phone() {
  local i
  for i in $(seq 1 20); do [ "$(phone_pane)" = "$2" ] && break; sleep 0.25; done
  eq "$1" "$(phone_pane)" "$2"
}
peval() { w pane eval "$P1" "$1" --on phone 2>&1; }
over() { local v; v="$(peval "$1")"; [ "${v%%.*}" -gt "$2" ] 2>/dev/null; }

ios_ui_start BrowserDragUITests/testBrowserKeepsSidewaysDrags

if step loaded; then
  check "the phone loads the browser page" poll 30 plog "browser pane $P1 loaded"
  w pane page "$P1" "http://localhost:$SPORT/two.html" --on phone >/dev/null
  check "the phone moved to /two.html, so there is a page to go back to" poll 20 sh -c "[ \"\$('$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane dom '$P1' h1 --on phone 2>&1)\" = '<h1>Board two</h1>' ]"
  expect_phone "the phone shows the browser pane $P1" "$P1"
  ack loaded
fi
if step dragged-left; then
  expect_phone "a right-to-left drag on the page left the phone on $P1" "$P1"
  check "the page saw the finger travel left more than 100 px (window.left = $(peval window.left))" over window.left 100
  ack dragged-left
fi
if step dragged-from-edge; then
  expect_phone "a drag from the left edge left the phone on $P1" "$P1"
  check "the page saw the finger travel right more than 100 px (window.right = $(peval window.right))" over window.right 100
  check "the edge drag did not go back: the page is still /two.html" sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane where '$P1' --on phone 2>&1 | grep -q 'two.html'"
  ack dragged-from-edge
fi
if step top-bar; then
  expect_phone "a drag along the top bar moved the phone to $P2" "$P2"
  check "phone.log has the top bar's swipe" plog "top bar: swipe left, page $P2"
  ack top-bar
fi
if step agent-swipe; then
  expect_phone "a swipe on the agent page moved the phone back to $P1" "$P1"
  ack agent-swipe
fi
ios_ui_finish browser-drag
