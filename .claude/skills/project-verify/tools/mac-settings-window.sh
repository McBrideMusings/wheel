#!/usr/bin/env bash
# § 2d Settings window (wheel-h3z5.7): the window resizes both ways down to a minimum, keeps its
# size across a relaunch, and each page's "Open" button opens the wheel.toml the page names (the
# machine file on the General page, the project's on the Project tab). Drives the real Mac app
# against this script's own daemon. The button runs with launch=false so no editor starts; the app
# log line "settings: open <path> for editing" is what a real press writes too. With WHEEL_SHOTS set
# it saves the window at two sizes to $WHEEL_SHOTS; run it with WHEEL_APPEARANCE=light, then dark.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
SPACE="$(mkspaces 1)"
prefs_reset
LOOK="${WHEEL_APPEARANCE:-system}"
size() { st '.settings.window_frame | "\(.width|round)x\(.height|round)"'; }
shot() { [ -z "${WHEEL_SHOTS:-}" ] || { app_post "/window/front?which=settings" >/dev/null; sleep 0.6; app_post "/window/snapshot?which=settings&path=$WHEEL_SHOTS/settings-$1-$LOOK.png" >/dev/null; }; }
resize() { app_post "/settings?action=resize&width=$1&height=$2" >/dev/null; sleep 0.5; }

launch_app
check "the app connects" wait_st '.connection == "connected"' 20
app_post "/settings?action=open" >/dev/null
check "settings opens" wait_st '.settings.open and .settings.loaded' 10
eq "the window is resizable" "$(st '.settings.window_resizable')" true
eq "its minimum width is 560" "$(st '.settings.window_min.width')" 560
MINH="$(st '.settings.window_min.height')"
check "its minimum height is at least the 380 of content" test "${MINH%.*}" -ge 380

resize 900 700; eq "grows to 900x700" "$(size)" 900x700
shot large
resize 640 520; eq "shrinks to 640x520" "$(size)" 640x520
shot small
resize 100 100
eq "below the minimum it stops at the minimum" "$(size)" "560x${MINH%.*}"

resize 820 610
quit_app; check "the app quit" test -z "$(app_pid)"
launch_app
check "the app reconnects" wait_st '.connection == "connected"' 20
app_post "/settings?action=open" >/dev/null
check "settings opens again" wait_st '.settings.open and .settings.loaded' 10
sleep 0.5
eq "the size survives a relaunch" "$(size)" 820x610
grep -q "settings window restored to 820x610" "$INST/WheelMac.log" && ok "the app log says it restored the size" || fail "restore log line" "missing"

app_post "/settings?action=open_file&layer=global&launch=false" >/dev/null
eq "General page's Open names the machine file" "$(st '.settings.opened_file.path')" "$INST/wheel.toml"
eq "…recorded without launching an editor" "$(st '.settings.opened_file.how')" "not launched"
app_post "/settings?action=tab&tab=project" >/dev/null
app_post "/settings?action=project&id=$SPACE" >/dev/null
check "the project rows resolve" wait_st ".settings.space == \"$SPACE\"" 10
app_post "/settings?action=open_file&layer=project&launch=false" >/dev/null
eq "Project tab's Open names the project's file" "$(st '.settings.opened_file.path')" "$SCR/proj1/wheel.toml"
eq "the state lists both files" "$(st '.settings.files | "\(.global) \(.project)"')" "$INST/wheel.toml $SCR/proj1/wheel.toml"
shot project
stop_daemon
finish
