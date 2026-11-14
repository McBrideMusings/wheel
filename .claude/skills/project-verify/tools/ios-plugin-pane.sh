#!/usr/bin/env bash
# § iOS plugin pane (wheel-pmf2.2.3): the phone runs a plugin pane's Luau script itself, draws it with WheelUI, and
# reloads it when the script is saved on the Mac, with the script's state kept; a broken save shows the error over the
# last good view.
#
# Needs WHEEL_IOS_APP: the built Wheel.app for the simulator (WheelPhone scheme), and a pool simulator.
# WHEEL_SHOTS=<dir> keeps ios-plugin-<counted|reloaded|broken|embed|typed>.png.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SIM=~/.claude/tools/sim
BUNDLE=com.piercemakes.wheel.phone
UDID=""
ios_cleanup() { [ -n "$UDID" ] && "$SIM" release >/dev/null 2>&1; cleanup; }
trap ios_cleanup EXIT

[ -d "${WHEEL_IOS_APP:-}" ] || { echo "set WHEEL_IOS_APP to the built Wheel.app (WheelPhone, iphonesimulator)" >&2; exit 2; }

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
use_fake_agent
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

# The project's own copy of the built-in sample package wins over the built-in, so saving it is a save on the Mac.
PROJ="$SCR/proj"
PKG="$PROJ/.wheel/plugins/sample"
mkdir -p "$PKG"
cp "$REPO/daemon/plugins/sample/wheel-plugin.toml" "$REPO/daemon/plugins/sample/main.luau" "$PKG/"
w space new "$PROJ" >/dev/null
PANE="$(w space ls --json | jget 0.panes.0)"
w pane kind "$PANE" plugin --plugin sample --plugin-pane counter >/dev/null

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
xcrun simctl install "$UDID" "$WHEEL_IOS_APP"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
PLOG="$DATA/Documents/phone.log"
rm -f "$PLOG"
plog() { grep -q -- "$1" "$PLOG" 2>/dev/null; }
shot() { [ -n "${WHEEL_SHOTS:-}" ] && mkdir -p "$WHEEL_SHOTS" && xcrun simctl io "$UDID" screenshot "$WHEEL_SHOTS/ios-plugin-$1.png" >/dev/null 2>&1; return 0; }

code="$(w remote pair --address 127.0.0.1)"
SIMCTL_CHILD_WHEEL_PAIR_JSON="$code" SIMCTL_CHILD_WHEEL_PLUGIN_PRESS="root.1,root.1" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
check "the phone pairs" poll 40 plog "connected to wss://127.0.0.1:$RPORT/ws"
check "the phone holds the plugin pane" poll 20 plog "plugin pane $PANE: sample/counter in "
check "the script loads with no error" poll 20 plog "plugin pane $PANE: sample/main.luau, reloads 0, error none, state {\"count\":0"
check "two presses count to 2" poll 20 plog "plugin pane $PANE: sample/main.luau, reloads 0, error none, state {\"count\":2"
sleep 1; shot counted

# A save on the Mac: wheeld pushes the new source, the phone reloads it, and the count stays 2.
sed -i '' 's/"Count "/"Total "/' "$PKG/main.luau"
check "a save reloads the pane with the count kept" poll 20 plog "plugin pane $PANE: sample/main.luau, reloads 1, error none, state {\"count\":2"
sleep 1; shot reloaded

# A broken save: the error line comes up and the last good view stays.
printf 'return {\n' >"$PKG/main.luau"
check "a broken save shows its error" poll 20 plog "plugin pane $PANE: sample/main.luau, reloads 1, error sample/main.luau"
sleep 1; shot broken

# Fixed again: the error goes, the count is still 2.
cp "$REPO/daemon/plugins/sample/main.luau" "$PKG/main.luau"
check "a fixed save clears the error with the count kept" poll 20 plog "plugin pane $PANE: sample/main.luau, reloads 2, error none, state {\"count\":2"

# The embed fixture (ADR-0042): a terminal pane of the space, a web view and the agent pane's chat inside the page.
# The phone draws the agent's conversation and the terminal's live screen, on a surface of the embed's own (wheel-pmf2.15).
mkdir -p "$PROJ/.wheel/plugins/embed"
cp "$REPO/tools/fixtures/plugins/embed/wheel-plugin.toml" "$REPO/tools/fixtures/plugins/embed/main.luau" "$PROJ/.wheel/plugins/embed/"
TPANE="$(w pane split "$PANE" right)"
w pane start "$TPANE" terminal >/dev/null 2>&1
# The term the pane runs, wherever the layout nests the pane.
TERM_ID="$(w status --json | python3 -c '
import json, sys
def walk(v):
    if isinstance(v, dict):
        kind = v.get("kind")
        if v.get("id") == sys.argv[1] and isinstance(kind, dict) and "term" in kind: print(kind["term"]); sys.exit()
        if v.get("id") == sys.argv[1] and "term" in v: print(v["term"]); sys.exit()
        for x in v.values(): walk(x)
    elif isinstance(v, list):
        for x in v: walk(x)
walk(json.load(sys.stdin))' "$TPANE")"
check "the terminal pane runs a term" test -n "$TERM_ID"
APANE="$(w pane split "$TPANE" down)"
w pane start "$APANE" agent --agent claude >/dev/null 2>&1
w agent send "$APANE" "hello from the mac" >/dev/null 2>&1
w pane kind "$PANE" plugin --plugin embed --plugin-pane main >/dev/null
check "the page runs the embed script" poll 20 plog "plugin pane $PANE: embed/main.luau, reloads 0, error none"
check "the page draws the agent chat embed of $APANE" poll 20 plog "plugin embed agent_chat $APANE agent"
check "the page draws one terminal surface for $TERM_ID" poll 20 plog "plugin embed terminal $PANE shows $TERM_ID: 1 embed surfaces"
check "the phone streams $TERM_ID to the embed" poll 20 plog "attached to $TERM_ID, replay [0-9]* bytes"
# While the space's tree changes the carousel's paged TabView holds extra copies of the page for a moment, each with a
# surface of its own; once the page settles one is left.
sleep 2
eq "one embed surface is left once the page settles" "$(grep -E "plugin embed terminal $PANE (shows|hid) $TERM_ID: " "$PLOG" | tail -1 | sed 's/.*: //')" "1 embed surfaces"
# The key bar scrolls inside the column instead of widening it: the embed lays out no wider than the screen less the
# fixture's 120 pt list column.
check "the embed sizes $TERM_ID from a page inside its column" poll 20 grep -Eq "surface $TERM_ID [0-9]+x[0-9]+ at .* in page (1[0-9]{2}|2[0-7][0-9])\.[0-9]+x" "$PLOG"
# Output the Mac's side types reaches the embed's screen: read back from the screenshot, not the log.
MARK="mac-says-$RANDOM"
w term send "$TERM_ID" "echo $MARK" --enter >/dev/null
check "the shell printed $MARK" w term read "$TERM_ID" --until "$MARK" --timeout 10
sleep 3
OCRSHOT="${WHEEL_SHOTS:-$SCR}/ios-plugin-embed.png"
mkdir -p "$(dirname "$OCRSHOT")"
xcrun simctl io "$UDID" screenshot "$OCRSHOT" >/dev/null 2>&1
check "the embed shows $MARK on screen" grep -q "$MARK" <(swift "$(dirname "${BASH_SOURCE[0]}")/ocr.swift" "$OCRSHOT" 2>/dev/null)

# Typing into the embed: the phone, relaunched on the plugin page, pastes a line and Return into the embed's surface.
xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1
TYPED="phone-typed-$RANDOM"
SIMCTL_CHILD_WHEEL_START_PAGE=1 SIMCTL_CHILD_WHEEL_TYPE="echo $TYPED" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
check "the phone types into the embed of $TERM_ID" poll 40 plog "typed into $TERM_ID in plugin pane $PANE: paste true, return true"
check "the shell got the typed line" w term read "$TERM_ID" --until "$TYPED" --timeout 10
sleep 3; shot typed

[ -n "${WHEEL_SHOTS:-}" ] && cp "$PLOG" "$WHEEL_SHOTS/ios-plugin-phone.log" && echo "phone.log $WHEEL_SHOTS/ios-plugin-phone.log"
[ "$FAIL" -eq 0 ] || { echo "--- phone.log"; grep -E "plugin|connected|layout" "$PLOG" | tail -30; echo "--- wheeld.log"; grep -E "plugin|remote|refused" "$(LOG)" | tail -20 | cut -c1-300; }
finish
