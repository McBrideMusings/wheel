#!/usr/bin/env bash
# § A saved URL registers as an artifact that opens in a browser pane (wheel-47eg): a `url` entry
# with `pane = true` is a row in the pane picker, and choosing it in an empty pane makes that pane a
# browser pane on the URL. Drives the real Mac app against this script's own daemon and a local
# HTTP server standing in for serve-sim. WHEEL_SHOTS=<dir> keeps the window snapshot.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS" "$SCR/site"
LOOK="${WHEEL_APPEARANCE:-light}"

printf '<!doctype html><title>serve-sim stand-in</title><h1>url entry page</h1>\n' >"$SCR/site/index.html"
SITE_PORT="$(pick_port)"
python3 -m http.server "$SITE_PORT" --bind 127.0.0.1 --directory "$SCR/site" >"$SCR/site.log" 2>&1 &
BG_PIDS+=("$!")
poll 20 curl -sf "http://127.0.0.1:$SITE_PORT/" >/dev/null
URL="http://127.0.0.1:$SITE_PORT/"

start_daemon --state-file "$SCR/spaces.json"
mkdir -p "$SCR/proj1"
S1="$(w space new "$SCR/proj1")"
w artifact new sim --page "$URL" --pane --space "$S1" >/dev/null
echo "     ls: $(w artifact ls --space "$S1" --json | jq -c "(.artifacts // .) | [.[] | {name, url}]")"
eq "ls --json carries the entry's url" "$(w artifact ls --space "$S1" --json | jq -r "(.artifacts // .) | .[] | select(.name == \"sim\") | .url")" "$URL"

prefs_reset
launch_app
wait_st '.windows | length >= 1' 20
w space focus "$S1" >/dev/null 2>&1 || true
P="$(w space ls | jq -r --arg s "$S1" '[.spaces[] | select(.id == $s)][0] | [.. | objects | select(.id? and .kind?) | .id] | first')"
w pane kind "$P" empty >/dev/null 2>&1 || true

app_post "/palette?pane=$P&action=open" >/dev/null
check "the picker is ready" wait_st ".panes[\"$P\"].palette.state == \"ready\"" 10
echo "     rows: $(st -c --arg p "$P" '[.panes[$p].palette.rows[] | {scope, name, problems}]')"
eq "the picker lists the url entry" "$(st -r --arg p "$P" '[.panes[$p].palette.rows[].name] | index("sim") != null')" "true"
eq "the url entry has no problems" "$(st -r --arg p "$P" '.panes[$p].palette.rows[] | select(.name == "sim") | .problems | length')" "0"
app_post "/palette?pane=$P&action=type&text=sim" >/dev/null
app_post "/palette?pane=$P&action=enter" >/dev/null
check "the pane becomes a browser pane" wait_st ".panes[\"$P\"].kind == \"browser\"" 10
echo "     pane: $(st -c --arg p "$P" '.panes[$p] | {kind, url, status, title, text}')"
eq "the daemon reports the pane as a browser on the url" "$(w space ls | jq -r --arg p "$P" '[.. | objects | select(.id? == $p)][0] | "\(.kind) \(.url)"')" "browser $URL"
check "the browser pane loaded the page" wait_st ".panes[\"$P\"].text | tostring | contains(\"url entry page\")" 15
app_post "/window/snapshot?path=$SHOTS/picker-url-entry-$LOOK.png" >/dev/null && echo "     shot $SHOTS/picker-url-entry-$LOOK.png"
finish
