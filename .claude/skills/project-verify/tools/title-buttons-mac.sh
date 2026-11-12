#!/usr/bin/env bash
# § 4 Title-row buttons in the Mac app (ADR-0037): a machine `[[title.button]]` draws at its end of
# the row with the state its command printed, a `hidden` entry removes the built-in of that id, a
# press runs the entry's command in the space's directory, and a state change moves no neighbour.
# With WHEEL_SHOTS set it saves the window.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

P1="$SCR/proj1"; mkdir -p "$P1"
start_daemon --state-file "$SCR/spaces.json"
cat >"$INST/wheel.toml" <<'TOML'
[[title.button]]
id = "vpn"
side = "left"
icon = "lock"
tip = "VPN"
command = "echo pressed > pressed.txt"
state = "if [ -f flip ]; then echo '{\"icon\":\"lock.open\",\"tint\":\"red\",\"badge\":\"9\"}'; else echo '{\"icon\":\"lock.fill\",\"tint\":\"green\",\"badge\":\"2\"}'; fi"
every = 1

[[title.button]]
id = "ci"
side = "right"
icon = "checkmark.seal"
tip = "CI"

[[title.button]]
id = "pin"
hidden = true
TOML
prefs_reset
w space new "$P1" >/dev/null
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 1)' 20
A="$(st '.windows[0].id')"
geo() { app_get /geometry; }
keys() { geo | jq -r --arg w "$A" '.windows[$w].title_buttons | keys | join(",")'; }
rect() { geo | jq -c --arg w "$A" --arg b "$1" '.windows[$w].title_buttons[$b]'; }
poll 10 sh -c "[ \"\$(curl -sf -H 'Authorization: Bearer $(TOKEN)' http://127.0.0.1:$DEBUG_PORT/geometry | jq -r '.windows[\"$A\"].title_buttons | has(\"plugin-vpn\")')\" = true ]"
eq "the row draws both plugin buttons, no pin, and keeps the other built-ins" "$(keys)" "mode,new-space,palette,plugin-ci,plugin-vpn,processes,session,sidebar"
LEFT="$(geo | jq -r --arg w "$A" '.windows[$w].title_buttons["plugin-vpn"][0] < .windows[$w].title_buttons["palette"][0]')"
eq "a left plugin sits left of the right-hand group" "$LEFT" "true"
RIGHT="$(geo | jq -r --arg w "$A" '.windows[$w].title_buttons | .["plugin-ci"][0] < .processes[0]')"
eq "a right plugin sits left of processes, inside the right group" "$RIGHT" "true"
BEFORE="$(geo | jq -c --arg w "$A" '.windows[$w].title_buttons | with_entries(select(.key != "plugin-vpn"))')"
VPN_BEFORE="$(rect plugin-vpn)"
touch "$P1/flip"
poll 8 sh -c "grep -q 'lock.open' \"$INST/WheelMac.log\""
sleep 1
AFTER="$(geo | jq -c --arg w "$A" '.windows[$w].title_buttons | with_entries(select(.key != "plugin-vpn"))')"
eq "the state flipped (icon, tint, badge) and no neighbour moved" "$AFTER" "$BEFORE"
eq "...the flipped button keeps its own rect" "$(rect plugin-vpn)" "$VPN_BEFORE"
check "the Mac log has the new state" grep -q 'vpn lock.open' "$INST/WheelMac.log"
pt="$(geo | jq -r --arg w "$A" '.windows[$w] as $x | $x.title_buttons["plugin-vpn"] as $r | "\($r[0] + $r[2] / 2 - $x.frame[0])&y=\($x.frame[1] + $x.frame[3] - ($r[1] + $r[3] / 2))"')"
app_post "/pane/click?window=$A&x=$pt" >/dev/null
poll 6 test -s "$P1/pressed.txt"
eq "a real click runs the command in the space's directory" "$(cat "$P1/pressed.txt" 2>/dev/null)" "pressed"
if [ -n "${WHEEL_SHOTS:-}" ]; then mkdir -p "$WHEEL_SHOTS"; app_post "/window/snapshot?window=$A&frame=1&path=$WHEEL_SHOTS/title-buttons.png" >/dev/null && echo "     shot $WHEEL_SHOTS/title-buttons.png"; fi
finish
