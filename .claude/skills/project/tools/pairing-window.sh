#!/usr/bin/env bash
# § 4 Pairing row and window: the copy and Show QR buttons, the floating window, the connect
# notice, the concealed clipboard item cleared after a minute. A phone is played by node's built-in
# WebSocket, which pairs with the one-time token exactly as the iPhone app does.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
command -v node >/dev/null || { echo "this check plays the phone with node" >&2; exit 2; }
trap 'app_cleanup; cleanup' EXIT

cat >"$SCR/phone.js" <<'EOF'
// usage: node phone.js <wss url> <pairing token> <device name>; prints the credential, then stays 3 s.
process.env.NODE_TLS_REJECT_UNAUTHORIZED = '0';
const [url, token, name] = process.argv.slice(2);
const ws = new WebSocket(`${url}?pair=${token}&name=${encodeURIComponent(name)}`);
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.ev === 'paired') { console.log(m.credential); setTimeout(() => process.exit(0), 3000); }
};
ws.onerror = () => { console.error('connection refused'); process.exit(1); };
EOF
phone() { # phone <device name>: pair using the code the app shows now
  local code; code="$(st '.pairing.code')"
  # The listener here is bound to loopback, so the phone dials that rather than the address the code names.
  node "$SCR/phone.js" "wss://127.0.0.1:$RPORT/ws" "$(printf %s "$code" | jq -r .token)" "$1" 2>/dev/null
}

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
mkspaces 2 >/dev/null
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 2)' 20
wait_st '.pairing.disabled != "Asking wheeld how phones reach it…"' 10

# Remote access off: both buttons are disabled and the row says why.
eq "with remote access off the row says why" "$(st .pairing.row)" "Remote access is off. Turn it on in Settings, Devices."
expect_exit 22 "copy is refused" curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" "http://127.0.0.1:$DEBUG_PORT/pairing?action=copy"
expect_exit 22 "Show QR code is refused" curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" "http://127.0.0.1:$DEBUG_PORT/pairing?action=show"

# Remote access on.
quit_app
stop_daemon
RPORT="$(pick_port)"
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"
launch_app
wait_st '.pairing.disabled == null' 20
eq "with remote access on the row names the address" "$(st .pairing.row | cut -c1-6)" "wss://"

# Show QR code: a floating window with only a close button.
app_post "/pairing?action=show" >/dev/null
check "Show QR code opens the window" wait_st '.pairing.window_visible' 5
check "the window holds a pairing code" wait_st '.pairing.code != null' 5
eq "it floats above the app's windows" "$(st .pairing.window_level_floating)" "true"
eq "its only button is close" "$(st -c '.pairing.window_buttons')" '{"close":true,"minimize":false,"zoom":false}'
eq "it waits for a device" "$(st .pairing.status)" "Waiting for a device to scan…"
app_post "/windows?action=new" >/dev/null
sleep 1
eq "focusing another window leaves it visible" "$(st .pairing.window_visible)" "true"

# A phone pairs: the window names it and closes within 3 seconds.
CRED="$(phone "Ada's iPhone")"
check "the phone paired and got a credential" test -n "$CRED"
eq "the window's events name the device" "$(st '.pairing.events | map(select(contains("Ada"))) | length > 0')" "true"
check "the window closed itself within 3 seconds" wait_st '.pairing.window_open | not' 3
eq "the Devices list has the phone" "$(w remote devices --json | jq -r '.[0].name')" "Ada's iPhone"

# With the switch off the window stays open after a phone connects.
app_post "/pairing?action=show" >/dev/null
wait_st '.pairing.window_visible and .pairing.code != null' 5
app_post "/pairing?action=switch&on=false" >/dev/null
phone "Second phone" >/dev/null
check "the window says who connected" wait_st '.pairing.connected == "Second phone"' 5
sleep 3
eq "with the switch off the window stays open" "$(st .pairing.window_open)" "true"
app_post "/pairing?action=details&on=true" >/dev/null
eq "details are open" "$(st .pairing.details_open)" "true"
eq "details list the connection events" "$(st '.pairing.events | map(select(contains("Second phone"))) | length > 0')" "true"
app_post "/pairing?action=close" >/dev/null

# Copy: a code on the clipboard as a concealed item, gone after a minute unless something else was copied.
printf 'before' | pbcopy
app_post "/pairing?action=copy" >/dev/null
check "copy puts a pairing code on the clipboard" wait_st '.pairing.clipboard.string | startswith("{")' 5
eq "the clipboard item is concealed" "$(st .pairing.clipboard.concealed)" "true"
eq "and the code is valid JSON with a token" "$(pbpaste | jq -r '.token | length')" "64"
sleep 62
eq "a minute later the clipboard is empty" "$(pbpaste)" ""
printf 'before' | pbcopy
app_post "/pairing?action=copy" >/dev/null
wait_st '.pairing.clipboard.string | startswith("{")' 5
printf 'something else' | pbcopy
sleep 62
eq "something copied since is left alone" "$(pbpaste)" "something else"
finish
