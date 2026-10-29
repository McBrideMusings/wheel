#!/usr/bin/env bash
# A terminal's drawn grid and its shell's size agree once a pane stops moving (wheel-h3z5.27).
# Two terminals side by side in the real Mac app; pane 1 runs a shell that prints `SZ <rows> <cols>`
# from its own tty on every SIGWINCH, so the last SZ line is the size wheeld gave the shell. After each
# layout change settles, that size must equal the app's grid for the pane (`/state` cols x rows).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
mkspaces 1 >/dev/null
prefs_reset

P1="$(w status --json | jq -r '[.. | objects | select(.kind? != null) | .id][0]')"
P2="$(w pane split "$P1" right)"
P3="$(w pane split "$P1" down)"
for p in $P1 $P2 $P3; do w pane kind "$p" terminal >/dev/null; done
ROOT="$(w space ls | jq -r '.spaces[0].root.id')"

launch_app
check "the app connects" wait_st '.connection == "connected" and (.windows | length) == 1' 20
WIN="$(st '.windows[0].id')"
app_post "/windows?action=frame&window=$WIN&x=40&y=100&w=1400&h=900" >/dev/null
for p in $P1 $P2 $P3; do
  check "pane $p has an attached terminal" wait_st ".panes[\"$p\"].attached == true and .panes[\"$p\"].cols != null" 30
done
sleep 2

# The shell in P1 is asked for its tty size with a fresh token each time; the token's output line
# (not its echoed input) carries the size wheeld gave the shell.
NONCE=0
shell_size() { # "<cols>x<rows>" from the shell's own tty
  NONCE=$((NONCE + 1))
  w term send "$P1" "printf 'RE%s%s\\n' S${NONCE}: \"\$(stty size)\"" --enter >/dev/null
  w term read "$P1" --until "RES${NONCE}:" --timeout 5 2>/dev/null | sed $'s/\x1b\\[[0-9;?]*[A-Za-z]//g' | tr -d '\r' | grep -a "^RES${NONCE}:" | tail -1 | awk '{print $2 "x" substr($1, index($1, ":") + 1)}'
}
app_size() { st ".panes[\"$P1\"] | \"\\(.cols)x\\(.rows)\""; }

agrees() { # <case name>: wait 1 s for the pane to settle, then the shell's size equals the drawn grid
  sleep 1
  local a s; a="$(app_size)"; s="$(shell_size)"
  eq "$1: shell $s equals drawn grid $a" "$s" "$a"
}

# 1. divider drag to a very short pane and back
app_post "/ratio?split=$ROOT&ratio=0.9" >/dev/null; agrees "divider to 0.9"
app_post "/ratio?split=$ROOT&ratio=0.2" >/dev/null; agrees "divider to 0.2"
app_post "/ratio?split=$ROOT&ratio=0.5" >/dev/null; agrees "divider back to 0.5"
for i in 1 2 3 4 5 6; do app_post "/ratio?split=$ROOT&ratio=0.$((3 + i))&held=1" >/dev/null; done
app_post "/ratio?split=$ROOT&ratio=0.5" >/dev/null; agrees "divider drag then drop"
# 2. window resize
app_post "/windows?action=frame&window=$WIN&x=40&y=100&w=900&h=700" >/dev/null; agrees "window 900x700"
app_post "/windows?action=frame&window=$WIN&x=40&y=100&w=1400&h=900" >/dev/null; agrees "window 1400x900"
# 3. zoom and unzoom
app_post "/focus?pane=$P1" >/dev/null
app_post "/action?id=zoom-pane" >/dev/null; agrees "zoomed"
app_post "/action?id=zoom-pane" >/dev/null; agrees "unzoomed"
# 4. sidebar toggle
app_post "/action?id=toggle-sidebar" >/dev/null; agrees "sidebar hidden"
app_post "/action?id=toggle-sidebar" >/dev/null; agrees "sidebar shown"
# 5. closing a neighbouring pane
w pane close "$P3" >/dev/null 2>&1; agrees "neighbour closed"
finish
