#!/usr/bin/env bash
# A terminal's drawn grid and its shell's size agree once a pane stops moving (wheel-h3z5.27).
# Two terminals side by side in the real Mac app; pane 1 runs a shell that prints `SZ <rows> <cols>`
# from its own tty on every SIGWINCH, so the last SZ line is the size wheeld gave the shell. After each
# layout change settles, that size must equal the app's grid for the pane (`/state` cols x rows).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
mkspaces 2 >/dev/null
SP2="$(w space ls | jq -r '.spaces[1].id')"
SP1="$(w space ls | jq -r '.spaces[0].id')"
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
app_size() { st ".panes[\"$P1\"] | \"\\(.grid_cols)x\\(.grid_rows)\""; }   # the surface's own grid
now_ms() { perl -MTime::HiRes=time -e 'printf "%d", time()*1000'; }

# The loop pane P2 prints a line as wide as its tty on every pass; its last line must be as wide as
# the grid the app draws for it.
loop_width() { # width of the loop pane's newest full line
  w term read "$P2" 2>/dev/null | sed $'s/\x1b\\[[0-9;?]*[A-Za-z]//g' | tr -d '\r' | grep -a 'x|$' | tail -1 | awk '{print length($0)}'
}
w term send "$P2" "while :; do printf '%*s|\\n' \$((\$(tput cols)-1)) x; sleep 0.1; done" --enter >/dev/null

T1="$(st ".panes[\"$P1\"].term")"
wheeld_size() { grep -a "term=$T1 .*resize\|term=$T1,.*resize\| term=$T1 " "$(LOG)" | grep -a resize | tail -1 | sed -n 's/.*cols=\([0-9]*\).*rows=\([0-9]*\).*/\1x\2/p'; }

agrees() { # <case name>: wheeld has the surface's final grid within 200 ms of the surface reaching it, and the shell and a width-dependent program agree
  local t0 n=0 final="" tapp="" twh="" a w t lag s
  t0="$(now_ms)"
  while [ "$n" -lt 150 ]; do
    a="$(app_size)"; w="$(wheeld_size)"; t=$(( $(now_ms) - t0 ))
    if [ "$a" != "$final" ]; then final="$a"; tapp="$t"; twh=""; fi
    [ -z "$twh" ] && [ "$w" = "$final" ] && twh="$t"
    n=$((n + 1)); sleep 0.02
    [ -n "$twh" ] && [ "$((t - tapp))" -gt 600 ] && break
  done
  if [ -z "$twh" ]; then fail "$1: wheeld got the surface's final grid $final" "wheeld holds '$(wheeld_size)'"; return; fi
  lag=$(( twh - tapp )); [ "$lag" -lt 0 ] && lag=0
  [ "$lag" -gt 220 ] && grep -a "surface $T1 " "$INST/WheelMac.log" | tail -12 | cut -c1-200 | sed 's/^/     app log: /'
  [ "$lag" -le 220 ] && ok "$1: wheeld had the final grid $final ${lag} ms after the surface reached it" || fail "$1: wheeld had the final grid $final ${lag} ms after the surface reached it" "over 200 ms (+20 ms sampling)"
  s="$(shell_size)"
  eq "$1: shell's stty size $s equals the surface grid $final" "$s" "$final"
  eq "$1: loop program's last line is as wide as pane P2's grid" "$(loop_width)" "$(st ".panes[\"$P2\"].grid_cols")"
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
# 5. space switch away and back
w space focus "$SP2" >/dev/null; sleep 1; w space focus "$SP1" >/dev/null; agrees "space switched away and back"
# 7. font size
app_post "/focus?pane=$P1" >/dev/null
press() { app_post "/key?keys=$(printf %s "$1" | sed 's/+/%2B/g')" >/dev/null; }
press cmd+equal; agrees "font larger"
press cmd+minus; press cmd+minus; agrees "font smaller"
press cmd+0; agrees "font reset"
# 8. a pane moved to another space and back
w pane move "$P3" --end-of "$SP2" >/dev/null; agrees "neighbour moved to another space"
w pane move "$P3" "$P1" down >/dev/null; agrees "neighbour moved back"
# 6. closing a neighbouring pane
w pane close "$P3" >/dev/null 2>&1; agrees "neighbour closed"
finish
