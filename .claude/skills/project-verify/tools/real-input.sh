#!/usr/bin/env bash
# Real pointer and keyboard input in the debug app (wheel-h3z5.31). /input/mouse and /input/key post
# CGEvents at the window server, /input/cursor reads NSCursor.current, /input/views finds a view's
# window point. Cases: (4) the routes refuse while the app is not active; (1) a move over a shown
# chat composer reports the I-beam, and still does while a hidden space holds a `cursor: zoom-in`
# page; (2) a click on a file link in a reply opens it, a press-and-drag starting on it selects text
# and opens nothing; (3) with Settings' search field or the command palette's field focused, typing
# "hello" lands there, not in the chat composer. The app must be able to become active (a login
# session); the check activates it and does not restore the previous app.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

export CLAUDE_CONFIG_DIR="$SCR/claude"
use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
mkdir -p "$SCR/alpha" "$SCR/beta/zoom"
printf 'let one = 1\n' >"$SCR/alpha/Main.swift"
printf '<html><body style="margin:0;cursor:zoom-in;min-height:100vh"><p>zoom page</p></body></html>\n' >"$SCR/beta/zoom/index.html"
printf '[artifacts.zoom]\npath = "zoom"\n' >"$SCR/beta/wheel.toml"
A="$(w space new "$SCR/alpha")"
B="$(w space new "$SCR/beta")"
PA="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
PB="$(w space ls | jq -r --arg s "$B" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
w pane start "$PA" agent >/dev/null
w pane kind "$PB" web --artifact zoom >/dev/null

launch_app
check "the app connects" wait_st '.connection == "connected" and (.windows | length) == 1' 30
WIN="$(st '.windows[0].id')"
app_post "/windows?action=frame&window=$WIN&x=40&y=100&w=1400&h=900&quiet=1" >/dev/null
app_post "/focus?space=$A" >/dev/null
check "the agent pane is up" wait_st ".panes[\"$PA\"].kind == \"agent\" and .panes[\"$PA\"].agent != null" 20
S="$(st --arg p "$PA" '.panes[$p].agent')"
check "B's page loaded" wait_st ".panes.$PB.loaded_at != null" 20
say() { curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" --data-binary "$1" "http://127.0.0.1:$DEBUG_PORT/agent?agent=$S" >/dev/null; }
say "See $SCR/alpha/Main.swift now"
check "the reply has a file link" wait_st ".panes[\"$PA\"].links.linked | length > 0" 20

rawpost() { curl -s -X POST -H "Authorization: Bearer $(TOKEN)" "http://127.0.0.1:$DEBUG_PORT$1"; }
getj() { curl -s -H "Authorization: Bearer $(TOKEN)" "http://127.0.0.1:$DEBUG_PORT$1"; }
# post: a POST that, when the route says the app lost the front place, takes it back once and repeats.
post() {
  local out; out="$(rawpost "$1")"
  case "$out" in
    *"not the active app"*) activate; sleep 0.5; out="$(rawpost "$1")";;
  esac
  case "$out" in *'"error"'*) echo "     refused: $1: $out" >&2;; esac
  printf '%s' "$out"
}
mouse() { post "/input/mouse?window=$WIN&$1"; }
cursor() { getj /input/cursor | jq -r .cursor; }

# 4. Not the active app (launch_app opens it in the background): every route refuses and says why.
eq "4: the app is not active after a background launch" "$(getj /input/cursor | jq -r .app_active)" "false"
eq "4: a mouse route refuses, saying why" "$(rawpost "/input/mouse?window=$WIN&action=move&x=300&y=300" | jq -r '.error | test("not the active app")')" "true"
eq "4: a key route refuses, saying why" "$(rawpost "/input/key?text=x" | jq -r '.error | test("not the active app")')" "true"
eq "4: the cursor route still answers" "$(getj /input/cursor | jq -r 'has("cursor")')" "true"

# The app asks to be active (/app/activate); macOS lets another app's front place stand against that, so
# this check then asks System Events to make the app's process frontmost, as a click on its Dock icon would.
activate() {
  app_post "/app/activate" >/dev/null
  [ "$(getj /input/cursor | jq -r .app_active)" = true ] && return 0
  local try
  for try in 1 2 3; do
    osascript -e "tell application \"System Events\" to set frontmost of (first process whose unix id is $(app_pid)) to true" >/dev/null 2>&1
    sleep 0.5
    [ "$(getj /input/cursor | jq -r .app_active)" = true ] && return 0
  done
  poll 10 sh -c "[ \"\$(curl -s -H 'Authorization: Bearer $(TOKEN)' http://127.0.0.1:$DEBUG_PORT/input/cursor | jq -r .app_active)\" = true ]"
}
activate
eq "an unnamed key is refused, never sent as A" "$(rawpost "/input/key?text=,&mods=cmd" | jq -r '.error | test("give code")')" "true"
eq "the app is active" "$(getj /input/cursor | jq -r .app_active)" "true"
sleep 1

# 1. The I-beam over a shown composer; still the I-beam with a hidden zoom-in page in another space.
COMPOSER="$(getj '/input/views?class=TextView' | jq -c '[.views[] | select(.h < 200 and .w > 100)] | sort_by(.y) | last')"
echo "     composer view: $COMPOSER"
CX="$(printf '%s' "$COMPOSER" | jq '.x + .w / 2')"; CY="$(printf '%s' "$COMPOSER" | jq '.y + .h / 2')"
mouse "action=move&x=100&y=100" >/dev/null
BEFORE="$(cursor)"
mouse "action=move&x=$CX&y=$CY" >/dev/null
sleep 0.3
eq "1: a move over the shown composer reports the I-beam" "$(cursor)" "iBeam"
eq "1: B's page is hidden" "$(st --arg p "$PB" '.panes[$p].view_hidden')" "true"
mouse "action=move&x=$CX&y=$(echo "$CY - 4" | bc)" >/dev/null
sleep 0.3
eq "1: still the I-beam with the zoom-in page hidden in the other space" "$(cursor)" "iBeam"
echo "     cursor away from the composer was: $BEFORE"

# 2. A plain click on the link opens it; a press-and-drag starting on it selects text.
ROW="$(st --arg p "$PA" '.panes[$p].links.linked[0].row')"
PROSE="$(getj "/transcript/drawn?agent=$S&row=$ROW" | jq -r .prose)"
OFF="$(printf '%s' "${PROSE%%Main.swift*}" | wc -m | tr -d ' ')"
LINK="$(getj "/transcript/point?agent=$S&row=$ROW&offset=$((OFF + 2))")"
LX="$(printf '%s' "$LINK" | jq .x)"; LY="$(printf '%s' "$LINK" | jq .y)"
END="$(getj "/transcript/point?agent=$S&row=$ROW&offset=$((OFF + 8))")"
EX="$(printf '%s' "$END" | jq .x)"
echo "     link at ($LX,$LY) to x=$EX"
eq "2: the link is under that point" "$(getj "/transcript/link?agent=$S&x=$LX&y=$LY" | jq -r '.url != null')" "true"
mouse "action=move&x=$LX&y=$LY" >/dev/null
echo "     cursor over the link: $(cursor)"
mouse "action=down&x=$LX&y=$LY" >/dev/null
for t in 1 2 3 4; do mouse "action=drag&x=$(echo "$LX + ($EX - $LX) * $t / 4" | bc -l)&y=$LY" >/dev/null; done
mouse "action=up&x=$EX&y=$LY" >/dev/null
sleep 0.5
eq "2: a drag from the link opens nothing" "$(st --arg p "$PA" '.panes[$p].links.click.pane')" "null"
pbcopy </dev/null
post "/input/key?text=c&mods=cmd" >/dev/null
sleep 0.3
COPIED="$(pbpaste)"
echo "     ⌘C after the drag copied: '$COPIED'"
eq "2: the drag selected text (a real ⌘C copies a part of the link's name)" "$(case "$COPIED" in ""|*[!A-Za-z.\ ]*) echo no;; *) printf '%s' 'Main.swift now' | grep -qF -- "$COPIED" && echo yes || echo no;; esac)" "yes"
mouse "action=click&x=$LX&y=$LY" >/dev/null
check "2: a plain click on the link opens it" wait_st ".panes[\"$PA\"].links.click.pane != null" 10
eq "2: it opened the file" "$(st --arg p "$PA" '.panes[$p].links.click.page')" "file://$SCR/alpha/Main.swift"

# 3. Typing lands in the focused Settings search field, and the palette's field, not the composer.
composer() { st --arg p "$PA" '.panes[$p].composer'; }
typeword() { local c; for c in h e l l o; do post "/input/key?text=$c" >/dev/null; done; sleep 0.4; }
app_post "/settings?action=open" >/dev/null
sleep 1
app_post "/settings?action=tab&tab=chat" >/dev/null
sleep 1
activate
SEARCH="$(getj '/input/views?window=settings&class=AppKitTextField' | jq -c '[.views[] | select(.class == "AppKitTextField" and .w > 300)] | first')"
echo "     settings search field: $SEARCH"
post "/input/mouse?window=settings&action=click&x=$(echo "$(printf '%s' "$SEARCH" | jq .x) + 20" | bc -l)&y=$(echo "$(printf '%s' "$SEARCH" | jq .y) + 8" | bc -l)" >/dev/null
typeword
eq "3: Settings' search field took hello" "$(st '.settings.chat.query + .settings.terminal.query')" "hello"
eq "3: the chat composer stayed empty" "$(composer)" ""
app_post "/settings?action=close" >/dev/null
sleep 0.5
activate
app_post "/palette?action=open" >/dev/null
sleep 0.5
typeword
eq "3: the command palette's field took hello" "$(st '.palette.query')" "hello"
eq "3: the chat composer stayed empty with the palette open" "$(composer)" ""
app_post "/palette?action=close" >/dev/null
finish
