#!/usr/bin/env bash
# § Terminal appearance (wheel-e916.12): `terminal.*` settings restyle the Ghostty surfaces of the
# space they resolve for, live, without restarting the shell. Drives the real Mac app against this
# script's own daemon: /state's terminal pane carries `appearance` (what the daemon resolved) and
# `ghostty_config` (what the space's controller runs with); `wheel config set` moves the font size
# and the grid's columns change while the shell keeps its counter; each theme is shot in both
# appearances (set SHOTS=<dir> to keep the PNGs).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
SPACE="$(mkspaces 1)"
prefs_reset
launch_app
check "the app connects" wait_st '.connection == "connected"' 20
check "the space has a pane" wait_st ".spaces | length > 0" 10
PANE="$(st '[.spaces[0].root | .. | objects | select(.kind? != null) | .id][0]' 2>/dev/null)"
[ -n "$PANE" ] && [ "$PANE" != null ] || PANE="$(w status --json | jq -r '[.. | objects | select(.kind? != null) | .id][0]')"
w pane kind "$PANE" terminal >/dev/null
check "the pane is a terminal with a surface" wait_st ".panes[\"$PANE\"].term != null and .panes[\"$PANE\"].attached == true" 20
TERM_ID="$(st ".panes[\"$PANE\"].term")"
w term send "$TERM_ID" 'printf "\033[31mred\033[0m \033[32mgreen\033[0m \033[34mblue\033[0m\n"; ls --color=always / 2>/dev/null || ls -G /; i=0; while true; do i=$((i+1)); echo tick$i; sleep 1; done' --key enter
check "the shell is counting" wait_st ".panes[\"$PANE\"].viewport_text | test(\"tick2\")" 15

eq "the default size is 14" "$(st ".panes[\"$PANE\"].appearance.font_size")" 14
check "ghostty runs at 14 pt" wait_st ".panes[\"$PANE\"].ghostty_config | test(\"font-size = 14\")" 10
COLS14="$(st ".panes[\"$PANE\"].cols")"

w config set --machine terminal.font.size 22 >/dev/null
check "font-size 22 reaches the controller within a second" wait_st ".panes[\"$PANE\"].ghostty_config | test(\"font-size = 22\")" 1
check "the grid is narrower at 22 pt" wait_st ".panes[\"$PANE\"].cols < $COLS14" 3
eq "the shell kept counting (same session)" "$(st ".panes[\"$PANE\"].viewport_text | test(\"tick2\")")" true

w config set --machine terminal.line_height 1.3 >/dev/null
w config set --machine terminal.cursor.style bar >/dev/null
w config set --machine terminal.cursor.blink false >/dev/null
w config set --machine terminal.font.family Menlo >/dev/null
check "line height, cursor and family reach the config" wait_st ".panes[\"$PANE\"].ghostty_config | test(\"adjust-cell-height = 30%\") and test(\"cursor-style = bar\") and test(\"cursor-style-blink = false\") and test(\"font-family = Menlo\")" 5

w config set --machine terminal.font.size 14 >/dev/null
w config set terminal.font.size 16 --space "$SPACE" >/dev/null
check "the project file wins over the machine file" wait_st ".panes[\"$PANE\"].ghostty_config | test(\"font-size = 16\")" 5
w config unset --machine terminal.font.family >/dev/null
for theme in default solarized gruvbox github; do
  w config set --machine terminal.theme "$theme" >/dev/null
  check "theme $theme is resolved" wait_st ".panes[\"$PANE\"].appearance.theme == \"$theme\"" 5
  sleep 1
  if [ -n "${SHOTS:-}" ]; then
    mkdir -p "$SHOTS"
    w pane shot "$PANE" "$SHOTS/terminal-$theme-${WHEEL_APPEARANCE:-system}.png"
  fi
done
check "the app log records the restyle" grep -q "terminal controller for .* restyled" "$(st .log_path)"

app_post "/settings?action=open" >/dev/null
app_post "/settings?action=tab&tab=terminal" >/dev/null
check "settings opens on the Terminal tab" wait_st '.settings.open and .settings.tab == "terminal"' 10
eq "the Terminal tab lists the six keys" "$(st '.settings.terminal.rows | length')" 6
app_post "/settings?action=terminal_query&text=cursor" >/dev/null
eq "search narrows to the cursor keys" "$(st '.settings.terminal.rows | join(",")')" "terminal.cursor.style,terminal.cursor.blink"
app_post "/settings?action=terminal_query&text=" >/dev/null
app_post "/settings?action=terminal_toml&on=true" >/dev/null
check "the toml view prints what the machine file sets" wait_st '.settings.terminal.toml | test("size = 14")' 5

# A ⌘+ zoom on the surface survives a terminal.* change (wheel-e916.19), and a pane moved to another
# space is restyled with that space's look.
press() { app_post "/key?keys=$(printf %s "$1" | sed 's/+/%2B/g')" >/dev/null; }
w config unset --machine terminal.theme >/dev/null
w config set --machine terminal.font.size 14 >/dev/null
w config unset terminal.font.size --space "$SPACE" >/dev/null
check "the surface is at 14 pt" wait_st ".panes[\"$PANE\"].font_points == 14" 5
w pane focus "$PANE" >/dev/null
check "the terminal holds the keyboard" wait_st ".first_responder == \"terminal $TERM_ID\"" 10
press cmd+equal; press cmd+equal
check "two ⌘+ presses zoom the surface to 16 pt" wait_st ".panes[\"$PANE\"].font_points == 16" 5
w config set --machine terminal.cursor.style underline >/dev/null
check "a terminal.* change reaches the controller" wait_st ".panes[\"$PANE\"].ghostty_config | test(\"cursor-style = underline\")" 5
eq "the zoom survives a terminal.* change" "$(st ".panes[\"$PANE\"].font_points")" 16
w config set --machine terminal.font.size 20 >/dev/null
check "a new terminal.font.size reaches the controller" wait_st ".panes[\"$PANE\"].ghostty_config | test(\"font-size = 20\")" 5
eq "the zoom survives a new font size" "$(st ".panes[\"$PANE\"].font_points")" 16
press cmd+0
check "⌘0 returns to the configured 20 pt" wait_st ".panes[\"$PANE\"].font_points == 20" 5

w config set terminal.font.size 12 --space "$SPACE" >/dev/null
mkdir -p "$SCR/proj-other"
SPACE2="$(w space new "$SCR/proj-other")"
check "the other space has its own pane" wait_st ".spaces | length == 2" 10
w pane move "$PANE" --end-of "$SPACE2" >/dev/null
check "the moved pane's surface is restyled with the new space's look" wait_st ".panes[\"$PANE\"].look_space == \"$SPACE2\" and .panes[\"$PANE\"].font_points == 20 and (.panes[\"$PANE\"].ghostty_config | test(\"font-size = 20\"))" 10
check "the rebuilt terminal holds the keyboard again" wait_st ".first_responder == \"terminal $TERM_ID\"" 10
check "the moved shell kept counting" wait_st ".panes[\"$PANE\"].viewport_text | test(\"tick\")" 10
check "the app log records the rebuild" grep -q "moved from space $SPACE to $SPACE2" "$(st .log_path)"

stop_daemon
finish
