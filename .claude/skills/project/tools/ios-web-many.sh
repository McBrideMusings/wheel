#!/usr/bin/env bash
# § iOS web pane, many files at once: a page of 40 images loads every one. The daemon refuses a
# client address its 16th open connection, so a page that opens a connection per file at once breaks
# the ones past the limit (`Network.NWError 54`); PageSchemeHandler lets 4 requests run at a time.
# Otherwise as ios-web.sh (same needs).
# (Derived from) § iOS web pane (wheel-20eq): a web pane on the phone loads its artifact from the Mac's remote
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
mkdir -p "$SITE/site"
python3 -I - "$SITE/site" <<'PY'
import sys, struct, zlib
d = sys.argv[1]
def png():
    raw = b"".join(b"\x00" + bytes([i % 256, 80, 160]) * 64 for i in range(64))
    def chunk(t, b): return struct.pack(">I", len(b)) + t + b + struct.pack(">I", zlib.crc32(t + b))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 64, 64, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
for n in range(40): open("%s/i%d.png" % (d, n), "wb").write(png())
open(d + "/index.html", "w").write("<!doctype html><title>t</title><body>" + "".join('<img src="i%d.png">' % n for n in range(40)) + "</body>")
PY
printf '[artifacts.site]\npath = "site"\n' >"$SITE/wheel.toml"
SID="$(w space new "$SITE")"
PANE="$(w space ls --json | jget 0.panes.0)"
w pane kind "$PANE" web --artifact site >/dev/null

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
check "the page finishes loading" poll 30 plog "web pane $PANE loaded, status 200"
loaded() { [ "$(grep -c "web pane $PANE page wheel-page://pages/artifacts/$PANE/i[0-9]*.png -> 200" "$PLOG")" -ge 40 ]; }
check "all 40 images answered 200" poll 60 loaded
check "no image request failed" sh -c "! grep -q 'web pane $PANE page .*png failed' '$PLOG'"
check "the daemon refused no connection at its limit" sh -c "! grep -q 'peer at its connection limit' \"$(LOG)\""
[ "$FAIL" -eq 0 ] || { echo "--- phone.log"; tail -40 "$PLOG"; echo "--- wheeld.log"; grep -E "artifact|remote|refused" "$(LOG)" | tail -20 | cut -c1-300; }
finish
