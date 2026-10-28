#!/usr/bin/env bash
# § Rich replies (wheel-e916.10.1): a reply typesets $…$ maths, draws a swatch beside a hex colour
# in inline code, and links an ISO 8601 timestamp to its local time. Drives the real Mac app
# through its debug server against this script's own daemon; the fake agent echoes the prompt, so
# the prompt is the reply. Window shots (cacheDisplay, never screencapture) go to $WHEEL_SHOTS
# (default: $SCR/shots), one per theme.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset

mkdir -p "$SCR/a"
A="$(w space new "$SCR/a")"
P="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
w pane start "$P" agent >/dev/null

w agent send "$P" 'Euler: $e^{i\pi}+1=0$. Costs $5 and $10. Brand `#0a84ff`, alpha `#ff453a80`, short `#fff`, not a colour `#12345`. Built 2026-10-28T16:04:58Z.' >/dev/null

launch_app TZ=America/New_York
check "the app connects" wait_st '.connection == "connected"' 20
check "the reply arrives" wait_st ".panes[\"$P\"].transcript | map(select(startswith(\"agent: echo\"))) | length == 1" 20
RICH="$(app_get /state | jq -c '.. | objects | select(has("rich")) | .rich' | head -1)"
eq "one maths expression, inline" "$(jq -c '.math' <<<"$RICH")" '[{"display":false,"latex":"e^{i\\pi}+1=0"}]'
eq "three swatches, lower-cased; five-digit hex is not one" "$(jq -c '.swatches' <<<"$RICH")" '["#0a84ff","#ff453a80","#fff"]'
eq "one timestamp" "$(jq -r '.times[0].iso' <<<"$RICH")" "2026-10-28T16:04:58Z"
# The date leads the time on any day but today.
check "its local time on a 12-hour clock" jq -e '.times[0].local | test("^(Oct 28, )?12:04pm EDT$")' <<<"$RICH"

shoot() {
  local theme
  for theme in light dark; do
    WHEEL_APPEARANCE=$theme
    app_post /app/quit >/dev/null 2>&1; poll 15 sh -c "! kill -0 $(app_pid) 2>/dev/null" || true
    launch_app TZ=America/New_York
    wait_st '.connection == "connected"' 20
    app_post "/focus?space=$A" >/dev/null
    app_post "/window/front" >/dev/null
    sleep 2
    app_post "/window/snapshot?path=$SHOTS/rich-replies-$theme.png" >/dev/null
    echo "shot $SHOTS/rich-replies-$theme.png"
  done
  unset WHEEL_APPEARANCE
}
shoot
finish
