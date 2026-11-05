#!/usr/bin/env bash
# § Performance: what agents working in hidden spaces cost the window on screen. SPACES spaces
# (default 6), each an agent pane; RUNNING of them (default 5, never the last) hold a turn whose
# tool runs for 300 s (the fake agent's "work slowly 300"), so their live line and tool row are
# running. The last space is on screen and idle. Records 4 s of idle and four sidebar toggles
# (POST /sidebar?open=toggle, 0.8 s apart) with POST /metrics, every POST with quiet=1, and keeps
# a window shot with the sidebar open and closed. A hidden space's running turn must not animate:
# idle main-thread CPU stays where it is with no turn running. Limits are in perf.md.
# WHEEL_SHOTS=<dir> keeps the shots and the table as perf-hidden-turns.txt.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SPACES="${SPACES:-6}"
RUNNING="${RUNNING:-5}"
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
for s in $(seq 1 "$SPACES"); do
  mkdir -p "$SCR/proj$s"
  S="$(w space new "$SCR/proj$s")"
  [ "$s" = 1 ] && FIRST_SPACE="$S"
  ALL+=("$S")
  A="$(w space ls | jq -r --arg s "$S" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
  w pane start "$A" agent >/dev/null
  if [ "$s" -le "$RUNNING" ] && [ "$s" -lt "$SPACES" ]; then
    (w agent send "$A" "work slowly 300" >/dev/null 2>&1 &)
  else
    w agent send "$A" "hello" >/dev/null
  fi
done

launch_app
check "the app connects" wait_st '.connection == "connected" and (.windows | length) == 1' 30
WIN="$(st '.windows[0].id')"
app_post "/windows?action=frame&window=$WIN&x=40&y=100&w=1400&h=900&quiet=1" >/dev/null
app_post "/focus?space=$S&quiet=1" >/dev/null
app_post "/window/front?quiet=1" >/dev/null
sleep 8

app_post "/metrics?action=start" >/dev/null; sleep 4; IDLE="$(app_post "/metrics?action=stop")"
# The working arcs the tabs and sidebar rows draw for the hidden spaces are on screen and turn;
# with the working rank's mark turned off, what is left is the hidden spaces' own work.
(unset WHEEL_PANE WHEEL_SPACE; w config set status.dots needs_you,stopped,idle --machine >/dev/null)
sleep 2
app_post "/metrics?action=start" >/dev/null; sleep 4; HIDDEN="$(app_post "/metrics?action=stop")"
(unset WHEEL_PANE WHEEL_SPACE; w config set status.dots needs_you,stopped,working,idle --machine >/dev/null)
sleep 2
app_post "/metrics?action=start" >/dev/null
for _ in 1 2 3 4; do app_post "/sidebar?open=toggle&quiet=1" >/dev/null; sleep 0.8; done
SIDEBAR="$(app_post "/metrics?action=stop")"
OPEN_ROWS="$(st '.sidebar.rows | length')"
app_post "/window/snapshot?frame=1&path=$SHOTS/hidden-turns-sidebar-open.png" >/dev/null
app_post "/sidebar?open=toggle&quiet=1" >/dev/null; sleep 1
CLOSED_TABS="$(st '.sidebar.tabs | length')"
app_post "/window/snapshot?frame=1&path=$SHOTS/hidden-turns-sidebar-closed.png" >/dev/null
# A switch with the tabs showing and one with the sidebar showing: each hides the transcript of
# the space it leaves, and must hide that transcript's own scroll view, never the tab strip (34 pt
# tall) or the sidebar's list (230 pt wide). The app logs each scroll view it hides.
app_post "/spacetap?space=$FIRST_SPACE&quiet=1" >/dev/null; sleep 2
app_post "/window/snapshot?frame=1&path=$SHOTS/hidden-turns-switched-tabs.png" >/dev/null
app_post "/sidebar?open=true&quiet=1" >/dev/null; sleep 1
app_post "/spacetap?space=$S&quiet=1" >/dev/null; sleep 2
app_post "/window/snapshot?frame=1&path=$SHOTS/hidden-turns-switched-sidebar.png" >/dev/null
HID="$(grep -c 'transcript table hidden' "$(st .log_path)")"
WRONG="$(grep 'transcript table hidden' "$(st .log_path)" | sed -E 's/.*\(([0-9.]+), ([0-9.]+), ([0-9.]+), ([0-9.]+)\).*/\3 \4/' | awk '$1 < 300 || $2 < 100' | wc -l | tr -d ' ')"

# What the window draws, not what it holds: ROUNDS rounds of a switch with the sidebar showing,
# the sidebar closed, and a switch with the tabs showing, each ending in a window shot. In each,
# every tab's frame (GET /geometry) must hold its drawn name and marks (tab-ink.swift counts the
# bright pixels in it); a tab strip hidden under its frames counts 0.
swiftc -O "$(dirname "${BASH_SOURCE[0]}")/tab-ink.swift" -o "$SCR/tab-ink" || { echo "tab-ink.swift did not compile" >&2; exit 2; }
ROUNDS="${ROUNDS:-8}"
BLANK=0
INKS=()
for r in $(seq 1 "$ROUNDS"); do
  app_post "/sidebar?open=true&quiet=1" >/dev/null; sleep 0.8
  app_post "/spacetap?space=${ALL[$(( (2 * r) % SPACES ))]}&quiet=1" >/dev/null; sleep 0.8
  app_post "/sidebar?open=false&quiet=1" >/dev/null; sleep 0.8
  app_post "/spacetap?space=${ALL[$(( (2 * r + 1) % SPACES ))]}&quiet=1" >/dev/null; sleep 1.5
  app_post "/window/snapshot?frame=1&path=$SHOTS/hidden-turns-round-$r.png" >/dev/null
  app_get /geometry >"$SCR/geometry-$r.json"
  inks="$("$SCR/tab-ink" "$SHOTS/hidden-turns-round-$r.png" "$SCR/geometry-$r.json" "$WIN")" || { echo "tab-ink failed on round $r" >&2; exit 2; }
  least="$(awk 'NR == 1 || $2 < m { m = $2 } END { print m + 0 }' <<<"$inks")"
  tabs="$(grep -c . <<<"$inks")"
  INKS+=("$least/$tabs")
  if [ "$least" -lt 100 ] || [ "$tabs" -ne "$SPACES" ]; then BLANK=$((BLANK + 1)); fi
done
echo "tab rounds, least ink per tab / tabs with frames: ${INKS[*]}"

field() { jq -r "$2" <<<"$1"; }
TABLE="$(
  echo "perf hidden turns: $SPACES spaces, $RUNNING hidden ones with a running turn"
  printf '%-34s %10s %10s\n' "" "idle 4 s" "4 sidebar"
  for m in "main thread CPU (ms):.main_cpu_ms | floor" "frames:.frames" "frame time mean (ms):.frame_ms.mean * 10 | floor / 10" "frames over 34 ms:.frame_ms.over_34ms"; do
    printf '%-34s %10s %10s\n' "${m%%:*}" "$(field "$IDLE" "${m#*:}")" "$(field "$SIDEBAR" "${m#*:}")"
  done
)"
TABLE="$TABLE
idle 4 s, working marks off: main thread $(field "$HIDDEN" '.main_cpu_ms | floor') ms"
echo "$TABLE"
echo "$TABLE" >"$SHOTS/perf-hidden-turns.txt"
echo "shots: $SHOTS/hidden-turns-sidebar-open.png $SHOTS/hidden-turns-sidebar-closed.png"
check "the sidebar lists every space ($OPEN_ROWS rows)" test "$OPEN_ROWS" -eq "$SPACES"
check "the title row lists every space ($CLOSED_TABS tabs)" test "$CLOSED_TABS" -eq "$SPACES"
check "idle 4 s with hidden running turns, their working arcs turning: main thread under 400 ms" test "$(field "$IDLE" '.main_cpu_ms | floor')" -le 400
check "idle 4 s with hidden running turns, working marks off: main thread under 250 ms" test "$(field "$HIDDEN" '.main_cpu_ms | floor')" -le 250
check "hidden spaces hid their transcripts' scroll views ($HID logged)" test "$HID" -ge 1
check "no hidden scroll view was the tab strip or the sidebar list ($WRONG were)" test "$WRONG" -eq 0
check "after the switches the title row lists every space" test "$(st '.sidebar.tabs | length')" -eq "$SPACES"
check "every tab draws in each of $ROUNDS sidebar/tab rounds ($BLANK rounds had a blank tab)" test "$BLANK" -eq 0
finish
