#!/usr/bin/env bash
# § 4 Grid motion (wheel-t4gx.1): what a layout change costs today. A 2x2 grid of terminals in the
# real Mac app takes a scripted divider drag (steps for 2.5 s, then the drop) and then a zoom
# toggle pair repeated for 2.5 s; the app's `/metrics` records the main thread's frame gaps, how
# often each pane's view body ran, and how many `termResize` frames went to wheeld. Prints one
# table. A second launch with WHEEL_SLOW_ANIMATIONS=4 proves the switch multiplies the duration.
# Set WHEEL_SHOTS=<dir> to keep the table as grid-motion.txt there.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
mkspaces 1 >/dev/null
prefs_reset

# The 2x2 grid: p1 | p2 as the root split, each side split top / bottom.
P1="$(w status --json | jq -r '[.. | objects | select(.kind? != null) | .id][0]')"
P2="$(w pane split "$P1" right)"
P3="$(w pane split "$P1" down)"
P4="$(w pane split "$P2" down)"
for p in $P1 $P2 $P3 $P4; do w pane kind "$p" terminal >/dev/null; done
ROOT="$(w space ls | jq -r '.spaces[0].root.id')"

launch_app WHEEL_SLOW_ANIMATIONS=
check "the app connects" wait_st '.connection == "connected" and (.windows | length) == 1' 20
WIN="$(st '.windows[0].id')"
app_post "/windows?action=frame&window=$WIN&x=40&y=100&w=1400&h=900" >/dev/null
for p in $P1 $P2 $P3 $P4; do
  check "pane $p has an attached terminal" wait_st ".panes[\"$p\"].attached == true and .panes[\"$p\"].cols != null" 30
done
sleep 2
app_post "/focus?pane=$P1" >/dev/null

# One recording: <seconds> of <driver function>, the report on stdout.
record() {
  app_post "/metrics?action=start" >/dev/null
  "$@"
  app_post "/metrics?action=stop"
}

drag_divider() { # 2.5 s of steps as fast as the app answers, ratio swinging 0.30-0.70 twice, then the drop at 0.5
  local t0 r
  t0="$(perl -MTime::HiRes=time -e 'printf "%.4f", time')"
  while r="$(perl -MTime::HiRes=time -e '$e = time - $ARGV[0]; exit 1 if $e > 2.5; printf "%.4f", 0.5 + 0.2 * sin($e / 2.5 * 6.2832 * 2)' "$t0")"; do
    app_post "/ratio?split=$ROOT&ratio=$r&held=1" >/dev/null
  done
  app_post "/ratio?split=$ROOT&ratio=0.5" >/dev/null
  sleep 0.3
}

zoom_toggles() { # on and off every 0.6 s for 2.5 s: four toggles
  local i
  for i in 1 2 3 4; do
    app_post "/pane/button?pane=$P1&button=zoom" >/dev/null
    sleep 0.6
  done
  sleep 0.1
}

resize_window() { # 2.5 s of 16 ms width steps run in the app (POST /windows?action=sweep), 300 pt either side of 1400
  app_post "/windows?action=sweep&window=$WIN" >/dev/null
  sleep 2.9
}

DRAG="$(record drag_divider)"
ZOOM="$(record zoom_toggles)"
RESIZE="$(record resize_window)"
eq "the drag ends unzoomed" "$(st '.zoomed')" null

field() { jq -r "$2" <<<"$1"; }
f1() { printf '%.1f' "$1"; }
# row <label> <jq filter> [f1]: the filter's value in each recording, rounded with f1 when asked.
row() {
  local label="$1" filter="$2" fmt="${3:-}" r v out=()
  for r in "$DRAG" "$ZOOM" "$RESIZE"; do
    v="$(field "$r" "$filter")"
    [ -n "$fmt" ] && v="$(f1 "$v")"
    out+=("$v")
  done
  printf '%-34s %14s %14s %14s\n' "$label" "${out[@]}"
}
TABLE="$(
  echo "grid motion: 2x2 terminals, divider drag 2.5 s + 4 zoom toggles + window resize 2.5 s (animation $(field "$ZOOM" .animation_ms) ms)"
  printf '%-34s %14s %14s %14s\n' "" "divider drag" "zoom toggles" "window resize"
  row "recorded (ms)" .duration_ms f1
  row "frames" .frames
  row "frame time mean (ms)" .frame_ms.mean f1
  row "frame time p50 (ms)" .frame_ms.p50 f1
  row "frame time p95 (ms)" .frame_ms.p95 f1
  row "frame time max (ms)" .frame_ms.max f1
  row "frames over 17 ms" .frame_ms.over_17ms
  row "frames over 34 ms" .frame_ms.over_34ms
  for p in $P1 $P2 $P3 $P4; do
    row "body evaluations, pane $p" ".bodies[\"$p\"] // 0"
  done
  row "body evaluations, all panes" .bodies_total
  row "termResize frames to wheeld" .term_resizes
)"
echo "$TABLE"
if [ -n "${WHEEL_SHOTS:-}" ]; then mkdir -p "$WHEEL_SHOTS"; echo "$TABLE" >"$WHEEL_SHOTS/grid-motion.txt"; echo "table: $WHEEL_SHOTS/grid-motion.txt"; fi

check "the drag drew frames" test "$(field "$DRAG" .frames)" -gt 30
check "a divider drag re-runs at most one body per pane" test "$(field "$DRAG" .bodies_total)" -le 4
check "four zoom toggles send one termResize each" test "$(field "$ZOOM" .term_resizes)" -le 4
eq "the zoom takes 220 ms at normal speed" "$(field "$ZOOM" .animation_ms)" 220
# A resize lays out the whole window on every step, so any view whose layout cost grows with the
# window's width (a strip measuring candidate rows, a transcript re-measuring rows) shows here.
check "a window resize keeps frames under 34 ms (at most 10 over)" test "$(field "$RESIZE" .frame_ms.over_34ms)" -le 10

# The slow switch: the same app with WHEEL_SLOW_ANIMATIONS=4 runs the zoom in 4 x 220 ms.
kill_app; DEBUG_PORT=""
launch_app WHEEL_SLOW_ANIMATIONS=4
check "the slow app connects" wait_st '.connection == "connected"' 20
SLOW="$(app_post "/metrics")"
eq "the slow switch reports its factor" "$(field "$SLOW" .slowdown)" 4
eq "the zoom takes 880 ms with WHEEL_SLOW_ANIMATIONS=4" "$(field "$SLOW" .animation_ms)" 880
finish
