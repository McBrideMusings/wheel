#!/usr/bin/env bash
# § Performance: what the spaces a window holds cost when the grid changes size. SPACES spaces
# (default 16), each an agent pane of REPLIES replies (default 40) and a terminal; the last one is
# on screen. Records four sidebar toggles (POST /sidebar?open=toggle, 0.8 s apart) and 2.5 s of
# window resize (POST /windows?action=sweep) with POST /metrics. Every POST passes quiet=1, so no
# state is built while the run is recorded. A hidden space must cost nothing: pane bodies and
# terminal resizes count the hidden spaces' work exactly; frames and duration show it as time.
# Limits are in perf.md. WHEEL_SHOTS=<dir> keeps the table as perf-spaces.txt.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SPACES="${SPACES:-16}"
REPLIES="${REPLIES:-40}"
MSG='## Reply N

Prose with `inline code`, a path apple/Mac/Sources/AppModel.swift:120, a missing one missing/fileN.swift:9 and **bold** text long enough to wrap across lines in a narrow pane.

```swift
let x = 1
```'

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
for s in $(seq 1 "$SPACES"); do
  mkdir -p "$SCR/proj$s"
  S="$(w space new "$SCR/proj$s")"
  A="$(w space ls | jq -r --arg s "$S" '.spaces[] | select(.id==$s) | .focus')"
  T="$(w pane split "$A" right)"
  w pane kind "$T" terminal >/dev/null
  w pane start "$A" agent >/dev/null
  for i in $(seq 1 "$REPLIES"); do w agent send "$A" "${MSG//N/$i}" >/dev/null; done
done

launch_app
check "the app connects" wait_st '.connection == "connected" and (.windows | length) == 1' 30
WIN="$(st '.windows[0].id')"
app_post "/windows?action=frame&window=$WIN&x=40&y=100&w=1400&h=900&quiet=1" >/dev/null
app_post "/focus?space=$S&quiet=1" >/dev/null
app_post "/window/front?quiet=1" >/dev/null
sleep 8

app_post "/metrics?action=start" >/dev/null
for _ in 1 2 3 4; do app_post "/sidebar?open=toggle&quiet=1" >/dev/null; sleep 0.8; done
SIDEBAR="$(app_post "/metrics?action=stop")"
app_post "/metrics?action=start" >/dev/null
app_post "/windows?action=sweep&window=$WIN&quiet=1" >/dev/null
sleep 2.9
RESIZE="$(app_post "/metrics?action=stop")"

field() { jq -r "$2" <<<"$1"; }
row() { printf '%-34s %14s %14s\n' "$1" "$(field "$SIDEBAR" "$2")" "$(field "$RESIZE" "$2")"; }
TABLE="$(
  echo "perf spaces: $SPACES spaces, each an agent pane of $REPLIES replies and a terminal"
  printf '%-34s %14s %14s\n' "" "4 sidebar" "resize"
  row "recorded (ms)" '.duration_ms | floor'
  row "frames" .frames
  row "frame time mean (ms)" '.frame_ms.mean * 10 | floor / 10'
  row "frames over 34 ms" .frame_ms.over_34ms
  row "pane body evaluations" .bodies_total
  row "termResize frames to wheeld" .term_resizes
)"
echo "$TABLE"
if [ -n "${WHEEL_SHOTS:-}" ]; then mkdir -p "$WHEEL_SHOTS"; echo "$TABLE" >"$WHEEL_SHOTS/perf-spaces.txt"; fi

check "four sidebar toggles re-run no hidden pane (at most 4 bodies)" test "$(field "$SIDEBAR" .bodies_total)" -le 4
check "four sidebar toggles resize only the terminal on screen (at most 8)" test "$(field "$SIDEBAR" .term_resizes)" -le 8
check "four sidebar toggles finish within 5 s" test "$(field "$SIDEBAR" '.duration_ms | floor')" -le 5000
check "a window resize re-runs no hidden pane (at most 4 bodies)" test "$(field "$RESIZE" .bodies_total)" -le 4
finish
