#!/usr/bin/env bash
# § 4 The title row (wheel-s7xj): with the sidebar hidden every space tab is the same width, at most
# 220 pt and at least 72 pt; past that the strip scrolls, and the sidebar, processes, ⌘K, pin and +
# buttons stay inside the window at any tab count. Reads the real frames from `GET /geometry`
# (`rows` per tab, `title_buttons` per button) and clicks each button with a real press and release
# (`POST /pane/click?x=&y=`). With WHEEL_SHOTS set it saves the window with 1, 3 and 10 tabs, at
# 1400 and 900 pt, and with the sidebar open, named after WHEEL_APPEARANCE.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
IDS=($(mkspaces 1))
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 1)' 20
A="$(st '.windows[0].id')"
MLOG="$INST/WheelMac.log"
LOOK="${WHEEL_APPEARANCE:-light}"

geo() { app_get /geometry; }
# size <width>: the window at that width and 700 pt tall, its top left at 100,100 from the screen's bottom left.
size() { app_post "/windows?action=frame&window=$A&x=100&y=100&w=$1&h=700" >/dev/null; sleep 0.6; }
# widths: every tab's width, space separated.
widths() { geo | jq -r --arg w "$A" '[.windows[$w].rows[] | .[2] | floor] | map(tostring) | join(" ")'; }
# inside: 1 when every title button lies inside the window's frame and right of the tab strip.
inside() {
  geo | jq -r --arg w "$A" '.windows[$w] as $x
    | ($x.frame[0] + $x.frame[2]) as $right
    | ([$x.title_buttons[] | .[0] + .[2]] | max) as $last
    | ([$x.title_buttons | to_entries[] | select(.key != "sidebar") | .value[0]] | min) as $first
    | (($x.tab_strip // [0,0,0,0]) | .[0] + .[2]) as $strip
    | if $last <= $right and $strip <= $first + 0.5 then 1 else 0 end'
}
shot() { [ -n "${WHEEL_SHOTS:-}" ] || return 0; mkdir -p "$WHEEL_SHOTS"; app_post "/window/snapshot?window=$A&frame=1&path=$WHEEL_SHOTS/title-row-$1-$LOOK.png" >/dev/null && echo "     shot $WHEEL_SHOTS/title-row-$1-$LOOK.png"; }
# click <button>: a real press and release at the middle of a title button.
click() {
  local pt
  pt="$(geo | jq -r --arg w "$A" --arg b "$1" '.windows[$w] as $x | $x.title_buttons[$b] as $r
    | "\($r[0] + $r[2] / 2 - $x.frame[0])&y=\($x.frame[1] + $x.frame[3] - ($r[1] + $r[3] / 2))"')"
  app_post "/pane/click?window=$A&x=$pt" >/dev/null
}

app_post "/windows?action=sidebar&window=$A&open=false" >/dev/null
size 1400
eq "one tab is the widest a tab gets" "$(widths)" "220"
eq "the title bar does not move the window; only the row's ground does" "$(st '.windows[0].movable')" "false"
eq "the five title buttons are measured" "$(geo | jq -r --arg w "$A" '.windows[$w].title_buttons | keys | join(",")')" "new-space,palette,pin,processes,sidebar"
eq "with one tab the buttons sit inside the window" "$(inside)" "1"
shot 1-tab

for d in 2 3; do mkdir -p "$SCR/proj$d"; w space new "$SCR/proj$d" >/dev/null; done
wait_st '.windows[0].spaces | length == 3' 10
sleep 0.4
eq "three tabs are each the widest a tab gets" "$(widths)" "220 220 220"
shot 3-tabs

# The row's empty middle is still the title bar: a double-click there does what the system's
# "Double-click a window's title bar" setting says (zoom, by default), and the window's frame changes.
BEFORE="$(geo | jq -c --arg w "$A" '.windows[$w].frame')"
app_post "/pane/click?window=$A&x=1000&y=20&count=2" >/dev/null
sleep 1
AFTER="$(geo | jq -c --arg w "$A" '.windows[$w].frame')"
check "a double-click on the row's empty middle zooms the window ($BEFORE -> $AFTER)" test "$BEFORE" != "$AFTER"
size 1400

for d in 4 5 6 7 8 9 10; do mkdir -p "$SCR/proj$d"; w space new "$SCR/proj$d" >/dev/null; done
wait_st '.windows[0].spaces | length == 10' 10
sleep 0.4
W1400="$(widths | tr ' ' '\n' | sort -u | tr '\n' ' ' | sed 's/ $//')"
check "at 1400 pt ten tabs share the strip, narrower than the widest and wider than the narrowest ($W1400)" test "$W1400" -lt 220 -a "$W1400" -gt 72
eq "at 1400 pt the buttons sit inside the window" "$(inside)" "1"
shot 10-tabs-1400

size 900
eq "at 900 pt ten tabs are each the narrowest a tab gets" "$(widths | tr ' ' '\n' | sort -u)" "72"
eq "at 900 pt the buttons still sit inside the window, right of the strip" "$(inside)" "1"
check "the app log says the strip scrolls" grep -q "title row ${A:0:8}: 10 tabs at 72 pt, scrolls=true" "$MLOG"
shot 10-tabs-900

# The last space's tab starts past the strip's right end; switching to it scrolls it into view.
ACTIVE="$(st '.windows[0].spaces[-1]')"
eq "the last tab starts outside the strip" "$(geo | jq -r --arg w "$A" --arg s "$ACTIVE" '.windows[$w] as $x | if $x.rows[$s][0] + $x.rows[$s][2] > $x.tab_strip[0] + $x.tab_strip[2] + 0.5 then 1 else 0 end')" "1"
app_post "/spacetap?space=$ACTIVE" >/dev/null
wait_st ".windows[0].active == \"$ACTIVE\"" 3
sleep 0.5
eq "the active tab is scrolled into the strip" "$(geo | jq -r --arg w "$A" --arg s "$ACTIVE" '.windows[$w] as $x | $x.rows[$s] as $r | $x.tab_strip as $t | if $r[0] >= $t[0] - 0.5 and $r[0] + $r[2] <= $t[0] + $t[2] + 0.5 then 1 else 0 end')" "1"

# Each button, clicked at 900 pt with ten tabs.
click palette
wait_st '.palette.open == true' 3
eq "⌘K opens the command palette" "$(st '.palette.mode')" "root"
app_post "/palette?action=close" >/dev/null
click new-space
wait_st '.palette.open == true' 3
eq "+ opens the new-space palette" "$(st '.palette.mode')" "new_space"
app_post "/palette?action=close" >/dev/null
click pin
wait_st '.windows[0].pinned == true' 3
eq "pin pins the window" "$(st '.windows[0].pinned')" "true"
click pin
wait_st '.windows[0].pinned == false' 3
click processes
poll 30 grep -q "processes card opened for space $ACTIVE" "$MLOG"
check "processes opens its card" grep -q "processes card opened for space $ACTIVE" "$MLOG"
app_post "/key?keys=esc" >/dev/null
click sidebar
wait_st '.sidebar.open == true' 3
sleep 0.5
eq "the sidebar button opens the sidebar" "$(st '.sidebar.open')" "true"
eq "with the sidebar open the buttons sit inside the window" "$(inside)" "1"
shot sidebar-open

finish
