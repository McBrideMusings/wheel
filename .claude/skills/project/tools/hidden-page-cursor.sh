#!/usr/bin/env bash
# A hidden space's web page must not set the cursor over the space on screen (wheel-h3z5.10).
# Space A (shown) holds an agent pane; space B (hidden, same window) holds an artifact page whose
# body is `cursor: zoom-in`. POST /cursor lists the page views that can set the cursor at a window
# point over A's pane (a view not hidden, nor under a hidden ancestor, whose frame holds the point;
# the person's real pointer is not used). Wanted: none while A is shown, the page hidden but
# mounted (view_hidden true, load_count unchanged), and B's page the only claim once B is shown.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
mkdir -p "$SCR/alpha" "$SCR/beta/zoom"
printf '<html><body style="margin:0;cursor:zoom-in;min-height:100vh"><p>zoom page</p></body></html>\n' >"$SCR/beta/zoom/index.html"
printf '[artifacts.zoom]\npath = "zoom"\n' >"$SCR/beta/wheel.toml"
A="$(w space new "$SCR/alpha")"
B="$(w space new "$SCR/beta")"
PA="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
PB="$(w space ls | jq -r --arg s "$B" '.spaces[] | select(.id==$s) | .focus')"
w pane start "$PA" agent >/dev/null
w pane kind "$PB" web --artifact zoom >/dev/null

launch_app
check "the app connects" wait_st '.connection == "connected" and (.windows | length) == 1' 30
WIN="$(st '.windows[0].id')"
app_post "/windows?action=frame&window=$WIN&x=40&y=100&w=1400&h=900&quiet=1" >/dev/null
app_post "/focus?space=$A" >/dev/null
check "A is shown" wait_st ".windows[0].active == \"$A\"" 10
check "B's page loaded" wait_st ".panes.$PB.loaded_at != null" 20
sleep 1

claims() { app_post "/cursor?x=300&y=300" | jq -c .claims; }
loads() { st --arg p "$PB" '.panes[$p].load_count'; }
LOADS="$(loads)"
eq "B's page is hidden while A is shown" "$(st --arg p "$PB" '.panes[$p].view_hidden')" "true"
eq "no page can set the cursor over A" "$(claims)" "[]"
eq "B's page was not reloaded" "$(loads)" "$LOADS"
check "a shot of the hidden page still draws it" w pane shot "$PB" "$SCR/hidden.png"
check "the shot is a PNG" test -s "$SCR/hidden.png"
eq "the page is hidden again after the shot" "$(st --arg p "$PB" '.panes[$p].view_hidden')" "true"

app_post "/focus?space=$B" >/dev/null
check "B is shown" wait_st ".windows[0].active == \"$B\"" 10
eq "B's page is visible once B is shown" "$(st --arg p "$PB" '.panes[$p].view_hidden')" "false"
eq "B's page was not reloaded by showing it" "$(loads)" "$LOADS"
eq "B's page can set the cursor over itself" "$(claims)" "[\"$PB\"]"
finish
