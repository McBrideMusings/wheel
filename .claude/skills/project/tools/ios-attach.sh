#!/usr/bin/env bash
# § iPhone composer attachments (wheel-r7tj): a file picked on the phone is uploaded to the Mac
# (`PUT /uploads/<pane>/<name>`), stored under the instance's `uploads/<pane>/`, and its Mac path
# goes into the message the agent gets. A HEIC becomes a JPEG on the phone, a chip's ✕ deletes the
# stored file, a file over 100 MB never leaves the phone, and closing the pane removes its uploads.
# `simctl` cannot tap, so `WHEEL_ATTACH`, `WHEEL_ATTACH_REMOVE` and `WHEEL_SEND` make the calls the
# pickers and buttons make. Screenshots go to WHEEL_SHOTS=<dir>.
#
# Needs WHEEL_IOS_APP: the built Wheel.app for the simulator (WheelPhone scheme), and a pool simulator.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SIM=~/.claude/tools/sim
BUNDLE=com.piercemakes.wheel.phone
UDID=""
ios_cleanup() { [ -n "$UDID" ] && "$SIM" release >/dev/null 2>&1; cleanup; }
trap ios_cleanup EXIT

[ -d "${WHEEL_IOS_APP:-}" ] || { echo "set WHEEL_IOS_APP to the built Wheel.app (WheelPhone, iphonesimulator)" >&2; exit 2; }
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"

# The files a person would pick: an iPhone photo (HEIC), a note, and one past the limit.
FILES="$SCR/files"
mkdir -p "$FILES"
sips -s format png -z 600 800 /System/Library/Desktop\ Pictures/*.heic --out "$FILES/src.png" >/dev/null 2>&1 \
  || sips -s format png -z 600 800 /System/Library/CoreServices/CoreTypes.bundle/Contents/Resources/GenericDocumentIcon.icns --out "$FILES/src.png" >/dev/null
sips -s format heic "$FILES/src.png" --out "$FILES/IMG_2041.HEIC" >/dev/null
printf 'a note from the phone\n' >"$FILES/notes.txt"
mkfile -n 101m "$FILES/huge.bin"

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

mkdir -p "$SCR/proj"
A="$(w space new "$SCR/proj")"
P="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
w pane start "$P" agent >/dev/null
UP="$INST/uploads/$P"

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
xcrun simctl install "$UDID" "$WHEEL_IOS_APP"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
PLOG="$DATA/Documents/phone.log"
rm -f "$PLOG"
plog() { grep -q -- "$1" "$PLOG" 2>/dev/null; }
launch() { xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1; : >"$PLOG"; env "$@" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null; }

code="$(w remote pair --address 127.0.0.1)"
launch SIMCTL_CHILD_WHEEL_PAIR_JSON="$code"
check "the phone pairs" poll 40 plog "connected to wss://127.0.0.1:$RPORT/ws"
check "8: a protocol-3 server offers attachments" poll 20 plog "server protocol 3, this app's 3; attachments offered"

# 1, 2, 4: a photo and a note, uploaded and shown as ready chips.
launch SIMCTL_CHILD_WHEEL_ATTACH="$FILES/IMG_2041.HEIC,$FILES/notes.txt"
check "1: the HEIC is converted to JPEG on the phone" poll 40 plog "converted IMG_2041.HEIC (public.heic"
check "1: the photo is stored on the Mac" poll 40 plog "upload IMG_2041.jpg: stored at $UP/1-IMG_2041.jpg"
check "2: the note is stored on the Mac" poll 40 plog "upload notes.txt: stored at $UP/1-notes.txt"
check "1: the stored photo is a JPEG" bash -c "file '$UP/1-IMG_2041.jpg' | grep -q 'JPEG image data'"
check "2: the stored note is the file unchanged" cmp -s "$FILES/notes.txt" "$UP/1-notes.txt"
check "4: both chips are ready" poll 20 plog "attachments in $P: IMG_2041.jpg=ready, notes.txt=ready"
has_log "upload stored pane=$P path=$UP/1-IMG_2041.jpg" "the daemon logged the photo's upload"
has_log "upload stored pane=$P path=$UP/1-notes.txt" "the daemon logged the note's upload"
sleep 2
xcrun simctl io "$UDID" screenshot "$SHOTS/attach-ready.png" >/dev/null 2>&1
echo "     shot $SHOTS/attach-ready.png"
xcrun simctl ui "$UDID" appearance dark >/dev/null 2>&1
sleep 1
xcrun simctl io "$UDID" screenshot "$SHOTS/attach-ready-dark.png" >/dev/null 2>&1
echo "     shot $SHOTS/attach-ready-dark.png"
xcrun simctl ui "$UDID" appearance light >/dev/null 2>&1

# 5, then send: the note's chip is taken back (its file deleted), the photo goes with the message.
launch SIMCTL_CHILD_WHEEL_ATTACH="$FILES/IMG_2041.HEIC,$FILES/notes.txt" SIMCTL_CHILD_WHEEL_ATTACH_REMOVE=2 SIMCTL_CHILD_WHEEL_SEND="what is in this photo"
check "5: the note is stored before its chip is removed" poll 40 plog "upload notes.txt: stored at $UP/2-notes.txt"
check "5: the removed chip's file is deleted on the Mac" poll 40 plog "upload 2-notes.txt deleted on the Mac"
check "5: it is gone from the pane's uploads" bash -c "[ -d '$UP' ] && [ ! -e '$UP/2-notes.txt' ]"
check "the message goes with one attachment" poll 40 plog "send to .*, 1 attachments"
check "1: the agent gets the text" poll 30 bash -c "$WHEEL --url ws://127.0.0.1:$PORT/ws --token-file $INST/token agent read $P | grep -q 'echo: what is in this photo'"
check "1: and the photo's Mac path after it" poll 5 bash -c "$WHEEL --url ws://127.0.0.1:$PORT/ws --token-file $INST/token agent read $P | grep -qF '\`$UP/2-IMG_2041.jpg\`'"

# Attachments alone, no text.
launch SIMCTL_CHILD_WHEEL_ATTACH="$FILES/notes.txt" SIMCTL_CHILD_WHEEL_SEND=""
check "a message of attachments alone goes out" poll 40 plog "send to .*, 1 attachments"
check "the agent gets the path alone" poll 30 bash -c "$WHEEL --url ws://127.0.0.1:$PORT/ws --token-file $INST/token agent read $P | grep -qF 'echo: \`$UP/2-notes.txt\`'"

# 6: over the limit, refused on the phone without reading it.
launch SIMCTL_CHILD_WHEEL_ATTACH="$FILES/huge.bin"
check "6: a file over 100 MB is refused on the phone" poll 40 plog "upload huge.bin refused here: 105906176 bytes is over 100 MB"
check "6: its chip shows it failed" poll 20 plog "attachments in $P: huge.bin=failed(\"over 100 MB\")"
sleep 1
xcrun simctl io "$UDID" screenshot "$SHOTS/attach-failed.png" >/dev/null 2>&1
echo "     shot $SHOTS/attach-failed.png"
check "6: nothing reached the Mac" bash -c "! ls '$UP' | grep -q huge"

# 7: closing the pane removes its uploads.
w pane close "$P" >/dev/null
check "7: closing the pane removes its uploads" poll 20 bash -c "[ ! -e '$UP' ]"
check "7: the daemon logged it" poll 10 grep -q "removed the pane's uploads" "$(LOG)"

[ "$FAIL" -eq 0 ] || { echo "--- phone.log"; tail -40 "$PLOG"; }
finish
