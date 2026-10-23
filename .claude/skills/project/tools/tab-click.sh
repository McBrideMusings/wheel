#!/usr/bin/env bash
# § 4 Switching spaces by click: `POST /spacetap` runs what a click on a tab or sidebar row runs
# (no double-click gesture waits in front of it), the app logs one `[tab] switch` line with the
# time from the press to the `windows:` line, and every switch is under 100 ms. Where on a tab a
# click lands (dot, padding, name, not the close button) is the view's hit area, checked by eye.
# Rename stays on the context menu's and accessibility action's path.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
IDS=($(mkspaces 3))
S1=${IDS[0]} S2=${IDS[1]} S3=${IDS[2]}
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 3)' 20
A="$(st '.windows[0].id')"
MLOG="$INST/WheelMac.log"

# timings <mode>: ten switches, each timed by the app from the press to the windows line.
timings() {
  local mode="$1" i target before ms all=()
  for i in $(seq 1 10); do
    target=$([ $((i % 2)) = 0 ] && echo "$S2" || echo "$S3")
    app_post "/spacetap?space=$S1" >/dev/null
    wait_st ".windows[0].active == \"$S1\"" 3
    before="$(grep -c '\[tab\] switch' "$MLOG")"
    app_post "/spacetap?space=$target" >/dev/null
    wait_st ".windows[0].active == \"$target\"" 3
    poll 20 sh -c "[ \"\$(grep -c '\\[tab\\] switch' '$MLOG')\" -gt $before ]"
    ms="$(grep '\[tab\] switch' "$MLOG" | tail -1 | sed -E 's/.*windows: ([0-9]+) ms.*/\1/')"
    all+=("$ms")
    eq "$mode: switch $i shows the clicked space" "$(st '.windows[0].active')" "$target"
  done
  echo "$mode: ms over 10 switches: ${all[*]}"
  check "$mode: every switch is under 100 ms" test "$(printf '%s\n' "${all[@]}" | sort -n | tail -1)" -lt 100
}

timings "sidebar row"
app_post "/windows?action=sidebar&window=$A&open=false" >/dev/null
sleep 0.5
timings "tab"

app_post "/rename?kind=space&id=$S1&action=begin" >/dev/null
eq "rename begins on a space" "$(st .renaming.space)" "$S1"
app_post "/rename?kind=space&id=$S1&action=commit&name=renamed" >/dev/null
wait_st '.renaming.space == null'
eq "and Return renames it in wheeld" "$(w space ls | jq -r --arg s "$S1" '.spaces[] | select(.id==$s) | .name')" "renamed"

finish
