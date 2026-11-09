#!/usr/bin/env bash
# § 4 Window mode in the Mac app: the mode button sits in the title row, a window's mode is its own
# and changes with the debug call and with the bound keys, the body switches between the spaces grid
# and the Desk, no other title-row button moves, and `[[title.button]] id = "mode" hidden = true`
# removes the button. With WHEEL_SHOTS set it saves the window in each mode.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

P1="$SCR/proj1"; mkdir -p "$P1"
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
w space new "$P1" >/dev/null
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 1)' 20
A="$(st '.windows[0].id')"
w_mode() { st '.windows[0].mode'; }
rects() { app_get /geometry | jq -c --arg w "$A" '.windows[$w].title_buttons'; }
others() { rects | jq -c 'del(.mode)'; }
eq "a new window shows spaces" "$(w_mode)" "spaces"
poll 10 sh -c "[ \"\$(curl -sf -H 'Authorization: Bearer $(TOKEN)' http://127.0.0.1:$DEBUG_PORT/geometry | jq -r '.windows[\"$A\"].title_buttons | has(\"mode\")')\" = true ]"
eq "the mode button is a 28 pt wide slot" "$(rects | jq -r '.mode[2]')" "28"
eq "it sits right of the session menu and left of the processes button" \
  "$(rects | jq -r '.session[0] < .mode[0] and .mode[0] < .processes[0]')" "true"
BEFORE="$(others)"
MODE_RECT="$(rects | jq -c '.mode')"
if [ -n "${WHEEL_SHOTS:-}" ]; then mkdir -p "$WHEEL_SHOTS"; app_post "/window/snapshot?window=$A&frame=1&path=$WHEEL_SHOTS/mode-spaces.png" >/dev/null && echo "     shot $WHEEL_SHOTS/mode-spaces.png"; fi
app_post "/windows?action=mode&window=$A&mode=desk" >/dev/null
wait_st '.windows[0].mode == "desk"' 6
eq "the debug call sets the window's mode to desk" "$(w_mode)" "desk"
eq "no other title-row button moved" "$(others)" "$BEFORE"
eq "the mode button kept its own rect" "$(rects | jq -c '.mode')" "$MODE_RECT"
if [ -n "${WHEEL_SHOTS:-}" ]; then app_post "/window/snapshot?window=$A&frame=1&path=$WHEEL_SHOTS/mode-desk.png" >/dev/null && echo "     shot $WHEEL_SHOTS/mode-desk.png"; fi
app_post "/key?keys=opt%2Bcmd%2B1" >/dev/null
wait_st '.windows[0].mode == "spaces"' 6
eq "opt+cmd+1 returns to spaces" "$(w_mode)" "spaces"
app_post "/key?keys=opt%2Bcmd%2B2" >/dev/null
wait_st '.windows[0].mode == "desk"' 6
eq "opt+cmd+2 shows the Desk" "$(w_mode)" "desk"
check "the Mac log records both changes" sh -c "grep -q 'mode desk' '$INST/WheelMac.log' && grep -q 'mode spaces' '$INST/WheelMac.log'"
cat >"$INST/wheel.toml" <<'TOML'
[[title.button]]
id = "mode"
hidden = true
TOML
poll 10 sh -c "[ \"\$(curl -sf -H 'Authorization: Bearer $(TOKEN)' http://127.0.0.1:$DEBUG_PORT/geometry | jq -r '.windows[\"$A\"].title_buttons | has(\"mode\")')\" = false ]"
eq "a hidden mode entry removes the button" "$(rects | jq -r 'has("mode")')" "false"
finish
