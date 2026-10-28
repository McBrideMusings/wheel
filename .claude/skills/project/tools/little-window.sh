#!/usr/bin/env bash
# § 4 Little windows: a space dragged onto the desktop, or opened from its menu, becomes a little
# window that belongs to its main window; four things bring it back; it survives a relaunch.
# Drives the real Mac app through its debug server against this script's own daemon. With
# WHEEL_SHOTS set it saves the preview card, the little window and the owner's placeholder
# (run it with WHEEL_APPEARANCE=light, then dark).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT
LOOK="${WHEEL_APPEARANCE:-light}"
shot() { [ -z "${WHEEL_SHOTS:-}" ] || { mkdir -p "$WHEEL_SHOTS"; app_post "/window/snapshot?$2&path=$WHEEL_SHOTS/little-$1-$LOOK.png" >/dev/null && echo "     shot $WHEEL_SHOTS/little-$1-$LOOK.png"; }; }

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
IDS=($(mkspaces 3))
S1=${IDS[0]} S2=${IDS[1]} S3=${IDS[2]}
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 3)' 20
A="$(st '.windows[0].id')"
app_post "/windows?action=frame&window=$A&x=40&y=200&w=1000&h=700" >/dev/null
P2="$(st --arg s "$S2" '.spaces[] | select(.id==$s) | .root.id')"
w pane kind "$P2" terminal >/dev/null
sleep 2

NAME2="$(basename "$SCR/proj2")"
little_count() { w window ls --json | jq '.little | length'; }
holds() { st --arg w "$A" --arg s "$1" '.windows[] | select(.id==$w) | .spaces | index($s) != null'; }

# 1. Drag a tab onto the desktop.
app_post "/spacedrag?action=start&space=$S2" >/dev/null
app_post "/spacedrag?action=hover&target=desktop" >/dev/null
eq "over the desktop the hint says a little window" "$(st '.space_drag.hint')" "Release to open “${NAME2}” in a little window"
check "the preview card is shown" wait_st '.space_drag.preview_card' 3
shot card "which=desktop"
app_post "/spacedrag?action=drop" >/dev/null
wait_st '.little_windows | length == 1 and .little_windows[0].open' 5
eq "wheel window ls lists one little window" "$(little_count)" "1"
eq "owned by the source window" "$(w window ls --json | jq -r '.little[0].owner')" "$A"
eq "for that space" "$(w window ls --json | jq -r '.little[0].space.id')" "$S2"
eq "the source window still lists the space" "$(holds "$S2")" "true"
eq "/state lists the little window open, 480 x 330" "$(st '.little_windows[0] | "\(.open) \(.frame[2])x\(.frame[3])"')" "true 480x330"
check "plain ls prints it with its owner" sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' window ls | grep -q '^little .* owner: $A\$'"

# 2. The owner's tab is dimmed; selecting it shows the placeholder.
app_post "/spacetap?space=$S2" >/dev/null
app_post "/window/front?window=${A:0:8}" >/dev/null
sleep 1.5
shot placeholder "window=${A:0:8}"
app_post "/windows?action=littlegoto&space=$S2" >/dev/null
sleep 1
shot window "which=little&window=$(st '.little_windows[0].id' | cut -c1-8)"

# 3. Four ways back, each leaving the space in its owner's tab.
back() { # back <endpoint> <label>
  app_post "$1" >/dev/null; wait_st '.little_windows | length == 0' 5
  eq "$2: no little window" "$(little_count)" "0"
  eq "$2: the owner still lists the space" "$(holds "$S2")" "true"
  app_post "/windows?action=little&space=$S2" >/dev/null; wait_st '.little_windows | length == 1' 5
  eq "$2: it opens again" "$(little_count)" "1"
}
back "/windows?action=littleback&space=$S2" "Bring Back, the tab's back icon and the window's back icon"
back "/windows?action=littleclosebutton&space=$S2" "the red light"

# 4. The right-click menu.
app_post "/windows?action=littleback&space=$S2" >/dev/null; wait_st '.little_windows | length == 0' 5
eq "the menu offers Open in Little Window" "$(st --arg s "$S2" '.space_menus[$s][] | select(.id=="little") | .title')" "Open in Little Window"
app_post "/space/menu?space=$S3&item=little" >/dev/null; wait_st '.little_windows | length == 1' 5
eq "the menu item opens an owned little window" "$(w window ls --json | jq -r '.little[0].owner')" "$A"
eq "and then offers Go to Little Window" "$(st --arg s "$S3" '.space_menus[$s][] | select(.id=="little") | .title')" "Go to Little Window"
eq "/state lists every space's menu while two are little" "$(st '.space_menus | length')" "3"

# 4b. A background space: `wheel space focus` opens it in a little window that belongs to none.
in_background() { w window ls --json | jq --arg s "$1" '.background | map(.id) | index($s) != null'; }
app_post "/windows?action=littleback&space=$S3" >/dev/null; wait_st '.little_windows | length == 0' 5
app_post "/windows?action=place&space=$S2&window=background" >/dev/null
wait_st ".background | index(\"$S2\") != null" 5
w space focus "$S2" >/dev/null
wait_st '.little_windows | length == 1 and .little_windows[0].open' 5
eq "space focus on a background space opens one little window" "$(little_count)" "1"
eq "that belongs to none" "$(w window ls --json | jq -r '.little[0].owner')" "null"
eq "for that space" "$(w window ls --json | jq -r '.little[0].space.id')" "$S2"
eq "and the space left the background" "$(in_background "$S2")" "false"
eq "the main window's spaces are unchanged" "$(st '.windows[0].spaces | length')" "2"
app_post "/windows?action=littleback&space=$S2" >/dev/null; wait_st '.little_windows | length == 0' 5
eq "the back button returns it to the background" "$(in_background "$S2")" "true"
# wheeld broadcasts the same active space for a re-focus as for any other report, so the app opens
# a little window only when the active space changes: focus another space first.
w space focus "$S3" >/dev/null; w space focus "$S2" >/dev/null; wait_st '.little_windows | length == 1' 5
app_post "/windows?action=littleclosebutton&space=$S2" >/dev/null; wait_st '.little_windows | length == 0' 5
eq "the red light returns it to the background" "$(in_background "$S2")" "true"
w space focus "$S3" >/dev/null
eq "space focus on a space in a main window opens no little window" "$(little_count)" "0"
app_post "/windows?action=place&space=$S2&window=$A" >/dev/null
app_post "/windows?action=little&space=$S3" >/dev/null; wait_st '.little_windows | length == 1' 5

# 5. A relaunch brings the little window back.
quit_app; launch_app
wait_st '.little_windows | length == 1 and .little_windows[0].open' 20
eq "the little window survives a relaunch" "$(little_count)" "1"
finish
