#!/usr/bin/env bash
# wheel-44rs: a press anywhere in a pane focuses it, whatever the pane shows. One click monitor
# watches the window's mouse-downs and maps each to the pane under it, so a page or a terminal that
# takes the press for itself still moves the focus. Each click is a real mouse event sent through
# the app's event path (`POST /pane/click`), at the middle of a pane's body or header.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT
LOOK="${WHEEL_APPEARANCE:-light}"
shot() { [ -z "${WHEEL_SHOTS:-}" ] || { mkdir -p "$WHEEL_SHOTS"; app_post "/window/snapshot?$2&path=$WHEEL_SHOTS/pane-click-$1-$LOOK.png" >/dev/null && echo "     shot $WHEEL_SHOTS/pane-click-$1-$LOOK.png"; }; }

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
mkdir -p "$SCR/alpha/site" "$SCR/beta"
printf '<html><body style="margin:0;height:100vh">page</body></html>\n' >"$SCR/alpha/site/index.html"
printf '[artifacts.site]\npath = "site"\n' >"$SCR/alpha/wheel.toml"
A="$(w space new "$SCR/alpha")"
launch_app
wait_st --arg s "$A" '.active == $s' 20
AGENT="$(st --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
app_post "/key?keys=1" >/dev/null
wait_st --arg p "$AGENT" '.panes[$p].kind == "agent"' 15

# agent | web / browser, then terminal | empty under the agent.
WEB="$(w pane split "$AGENT" right)"; w pane kind "$WEB" web --artifact site >/dev/null
BROWSER="$(w pane split "$WEB" down)"; w pane kind "$BROWSER" browser --page https://example.com >/dev/null
TERM_P="$(w pane split "$AGENT" down)"; w pane kind "$TERM_P" terminal >/dev/null
EMPTY="$(w pane split "$TERM_P" right)"
wait_st --arg w "$WEB" --arg b "$BROWSER" '.panes[$w].kind == "web" and .panes[$b].kind == "browser"' 15
sleep 1.5

focus_of() { w space ls | jq -r --arg s "$1" '.spaces[] | select(.id==$s) | .focus'; }
focus_ops() { grep -c 'layout op: {"op":"focus","pane"' "$INST/WheelMac.log"; }
click() { app_post "/pane/click?$1" | jq -r .pane; }

# Start from the empty pane, then click each other kind's body, then each kind's header.
w pane focus "$EMPTY" >/dev/null
shot before ""
for at in body header; do
  for p in "$AGENT" "$WEB" "$BROWSER" "$TERM_P" "$EMPTY"; do
    kind="$(st --arg p "$p" '.panes[$p].kind')"
    eq "the click on the $kind pane's $at lands on it" "$(click "pane=$p&at=$at")" "$p"
    check "a click on the $kind pane's $at focuses it" poll 5 sh -c "[ \"\$('$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' space ls | jq -r '.spaces[] | select(.id==\"$A\") | .focus')\" = '$p' ]"
    [ "$p" = "$WEB" ] && [ "$at" = body ] && shot web-clicked ""
  done
done
check "the app logs each click with its pane" grep -q "pane click $BROWSER" "$INST/WheelMac.log"

# A click on the pane that already holds the focus sends nothing.
BEFORE="$(focus_ops)"
click "pane=$EMPTY" >/dev/null
sleep 1
eq "a click on the focused pane sends no focus op" "$(focus_ops)" "$BEFORE"

# The command palette over the grid: a click under it leaves the focus where it was.
app_post "/palette?action=open&mode=root" >/dev/null
wait_st '.palette != null' 5
click "pane=$WEB" >/dev/null
sleep 1
eq "with the palette open a click leaves the focus" "$(focus_of "$A")" "$EMPTY"
check "the app logs why it ignored the click" grep -q "pane click ignored: the command palette is open" "$INST/WheelMac.log"
app_post "/palette?action=close" >/dev/null
wait_st '.palette == null' 5

# A zoomed pane: the others sit hidden under it, so a click where one was lands on the zoomed one.
AT="$(app_post "/pane/click?pane=$WEB" | jq -r '"x=\(.x)&y=\(.y)"')"
w pane focus "$AGENT" >/dev/null
sleep 0.5
app_post "/pane/button?pane=$AGENT&button=zoom" >/dev/null
wait_st --arg p "$AGENT" '.zoomed == $p' 5
sleep 1
eq "while the agent is zoomed a click where the page was lands on the agent" "$(click "$AT")" "$AGENT"
sleep 1
eq "...and leaves the focus on it" "$(focus_of "$A")" "$AGENT"
app_post "/pane/button?pane=$AGENT&button=zoom" >/dev/null
wait_st '.zoomed == null' 5

# A little window: a click in its other pane focuses that pane.
B="$(w space new "$SCR/beta")"
BT="$(focus_of "$B")"; w pane kind "$BT" terminal >/dev/null
BW="$(w pane split "$BT" right)"
app_post "/windows?action=little&space=$B" >/dev/null
wait_st '.little_windows | length == 1' 10
sleep 1.5
LITTLE="little-$(st '.little_windows[0].id' | cut -c1-8)"
w pane focus "$BT" >/dev/null
sleep 0.5
eq "a click in the little window lands on its empty pane" "$(click "pane=$BW&window=$LITTLE")" "$BW"
check "...and focuses it" poll 5 sh -c "[ \"\$('$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' space ls | jq -r '.spaces[] | select(.id==\"$B\") | .focus')\" = '$BW' ]"
finish
