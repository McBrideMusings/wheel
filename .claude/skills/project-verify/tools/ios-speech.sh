#!/usr/bin/env bash
# Settings › Speech on the phone (wheel-7w13): the tab opens beside Global, Project and Devices,
# lists Parakeet v3, Parakeet v2 and Silero VAD with size, source, folder and whether each is on
# the phone, and the picker shows the choice kept in UserDefaults (`speech.stt`) across a relaunch.
# Only Silero VAD (1.1 MB) downloads, through a staging folder, and is then removed. Installs the built WheelPhone into a pool
# simulator, pairs it with this script's own daemon, and reads the app's phone.log. Shots (simctl
# io) go to $WHEEL_SHOTS (default: $SCR/shots).
#
# Needs WHEEL_IOS_APP: the built Wheel.app for the simulator (WheelPhone scheme).
# WHEEL_SPEECH_BEFORE=1 shoots only the Global tab, for a build that has no Speech tab.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SIM=~/.claude/tools/sim
BUNDLE=com.piercemakes.wheel.phone
UDID=""
ios_cleanup() { [ -n "$UDID" ] && "$SIM" release >/dev/null 2>&1; cleanup; }
trap ios_cleanup EXIT

[ -d "${WHEEL_IOS_APP:-}" ] || { echo "set WHEEL_IOS_APP to the built Wheel.app (WheelPhone, iphonesimulator)" >&2; exit 2; }
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"
mkdir -p "$SCR/a"
w space new "$SCR/a" >/dev/null

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
xcrun simctl install "$UDID" "$WHEEL_IOS_APP"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
PLOG="$DATA/Documents/phone.log"
xcrun simctl ui "$UDID" appearance dark
# The choice lives outside the app container, so a reinstall keeps an earlier run's: start from none.
xcrun simctl spawn "$UDID" defaults delete "$BUNDLE" speech.stt >/dev/null 2>&1

launch() { # launch <settings tab>: (re)start the app paired, on that Settings tab
  xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1
  local code; code="$(w remote pair --address 127.0.0.1)"
  SIMCTL_CHILD_WHEEL_PAIR_JSON="$code" SIMCTL_CHILD_WHEEL_SETTINGS="$1" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
}
plog() { grep -q -- "$1" "$PLOG" 2>/dev/null; }
wait_plog() { poll "${2:-30}" plog "$1"; }
shoot() { sleep 2; xcrun simctl io "$UDID" screenshot "$SHOTS/$1.png" >/dev/null 2>&1; echo "shot $SHOTS/$1.png"; }

launch global
check "the phone pairs" wait_plog "paired with wss://127.0.0.1:$RPORT/ws" 40
check "Settings opens on Global" wait_plog "open settings Global" 30
shoot settings-global
if [ "${WHEEL_SPEECH_BEFORE:-}" = 1 ]; then finish; fi

launch speech
check "Settings opens on Speech" wait_plog "open settings Speech" 30
shoot settings-speech-v3
check "with no choice saved, v3 is used" sh -c "! xcrun simctl spawn $UDID defaults read $BUNDLE speech.stt 2>/dev/null"

xcrun simctl spawn "$UDID" defaults write "$BUNDLE" speech.stt v2
launch speech
check "Settings opens on Speech again" poll 30 sh -c "[ \"\$(grep -c 'open settings Speech' '$PLOG')\" -ge 2 ]"
check "the choice is kept in UserDefaults" sh -c "xcrun simctl spawn $UDID defaults read $BUNDLE speech.stt | grep -qx v2"
shoot settings-speech-v2
check "no model downloaded before asked" sh -c "! grep -q 'speech settings: downloading' '$PLOG'"

# Silero VAD (1.1 MB) downloads through a staging folder and lands whole; Remove deletes it.
launch_do() { # launch_do <action>: relaunch on Speech running one Settings › Speech action
  xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1
  local code; code="$(w remote pair --address 127.0.0.1)"
  SIMCTL_CHILD_WHEEL_PAIR_JSON="$code" SIMCTL_CHILD_WHEEL_SETTINGS=speech SIMCTL_CHILD_WHEEL_SPEECH_DO="$1" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
}
launch_do download:vad
check "Silero VAD downloads" wait_plog "speech settings: Silero VAD v6 downloaded" 90
shoot settings-speech-vad
check "it lands in the models folder, not the staging one" sh -c "find '$DATA' -type d -path '*Models/silero-vad' -not -path '*staging*' | grep -q . && ! find '$DATA' -name '.staging-vad' | grep -q ."
launch_do remove:vad
check "Remove deletes it" wait_plog "speech settings: removed Silero VAD v6" 30
check "its folder is gone" sh -c "! find '$DATA' -type d -path '*Models/silero-vad' | grep -q ."
finish
