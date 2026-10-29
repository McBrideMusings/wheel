#!/usr/bin/env bash
# § Performance: what the Mac app costs at rest and while an agent replies. SPACES spaces (default 6), each
# with an agent pane holding REPLIES markdown replies (default 40: headings, links, file paths,
# code, a table) and a terminal, against the fake agent. Measures, after launch:
#   settle_s     seconds until the app's CPU first reads under 10% (ps, once a second, up to 90 s)
#   idle_cpu     mean CPU over the 10 s after that
#   busy_cpu     mean CPU over the last 20 s of the activity below
#   requests     the Mac app's messages to wheeld per op, per minute, while the agent on screen
#                replies every 2 s and its composer takes ten keys a second, from `status`'s `requests`; a client that asks again on every
#                redraw shows here
# Prints one table; limits are in perf.md. WHEEL_SHOTS=<dir> keeps it as perf-chat.txt.
#
# perf-chat.sh --live reads the installed app and the running wheeld as they are, over
# LIVE_WINDOW_S (default 60) seconds: the app's share of one core and the daemon's file_peek lines
# a minute. Run it after every client deploy; real sessions stream more than the fake agent does.
if [ "${1:-}" = --live ]; then
  WINDOW_S="${LIVE_WINDOW_S:-60}"
  pid="$(pgrep -x Wheel | head -1)"
  [ -n "$pid" ] || { echo "no Wheel app running" >&2; exit 2; }
  log="$HOME/Library/Logs/wheel/wheeld.log"
  # cputime as M:SS.ss or H:MM:SS, in seconds.
  cpu_s() { ps -o time= -p "$pid" | awk -F: '{ s = 0; for (i = 1; i <= NF; i++) s = s * 60 + $i; print s }'; }
  peeks() { grep -c 'file_peek' "$log" 2>/dev/null || echo 0; }
  c0="$(cpu_s)"; p0="$(peeks)"; sleep "$WINDOW_S"; c1="$(cpu_s)"; p1="$(peeks)"
  pct="$(awk -v a="$c0" -v b="$c1" -v w="$WINDOW_S" 'BEGIN { printf "%.1f", (b - a) * 100 / w }')"
  per_min=$(( (p1 - p0) * 60 / WINDOW_S ))
  echo "live: Wheel (pid $pid) ${pct}% of one core over ${WINDOW_S}s, ${per_min} file_peek/min"
  rc=0
  awk -v p="$pct" 'BEGIN { exit !(p < 10) }' || { echo "FAIL the app uses ${pct}% of one core, over 10%"; rc=1; }
  [ "$per_min" -lt 120 ] || { echo "FAIL the daemon answers ${per_min} file_peek a minute, over 120"; rc=1; }
  [ "$rc" -eq 0 ] && echo "ok   the live app is quiet"
  exit "$rc"
fi
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SPACES="${SPACES:-6}"
REPLIES="${REPLIES:-40}"
MSG='## Reply N

Prose with `inline code`, a path apple/Mac/Sources/AppModel.swift:120, a missing one missing/fileN.swift:9 and **bold** text long enough to wrap across lines in a narrow pane.

- an item with a link https://example.com/a/b
- an item with `code`

```swift
let x = 1
print(x + 2)
```

| a | b |
|---|---|
| 1 | 2 |'

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
check "the app connects" wait_st '.connection == "connected"' 30
PID="$(app_pid)"
app_post "/focus?space=$S" >/dev/null
app_post "/window/front" >/dev/null
cpu() { ps -o %cpu= -p "$PID" | tr -d ' '; }

t0="$(date +%s)"; settle=""
for _ in $(seq 1 90); do
  c="$(cpu)"
  if awk -v c="$c" 'BEGIN { exit !(c < 10) }'; then settle=$(( $(date +%s) - t0 )); break; fi
  sleep 1
done
[ -n "$settle" ] || settle=90

sum=0
for _ in $(seq 1 10); do sum="$(awk -v a="$sum" -v b="$(cpu)" 'BEGIN { print a + b }')"; sleep 1; done
idle="$(awk -v s="$sum" 'BEGIN { printf "%.1f", s / 10 }')"

# Activity: for 36 s the agent in the last space made (the one on screen) replies every 2 s and a
# person types ten characters a second into its composer (POST /agent/typing).
# Only the Mac app's own requests count (`mac <op>` keys), over the last 20 s, past the 15 s a
# missing file waits before it may be asked about again.
reqs() { w status --json | jq -c '.requests // {} | with_entries(select(.key | startswith("mac ")))'; }
SESS="$(st --arg p "$A" '.panes[$p].session')"
app_post "/agent/typing?session=$SESS&chars=360&per_second=10" >/dev/null
for i in $(seq 1 18); do
  w agent send "$A" "${MSG//N/live $i}" >/dev/null
  [ "$i" = 8 ] && R0="$(reqs)" && A0="$(date +%s)" && busy_sum=0 && busy_n=0
  sleep 1
  if [ -n "${A0:-}" ]; then busy_sum="$(awk -v a="$busy_sum" -v b="$(cpu)" 'BEGIN { print a + b }')"; busy_n=$((busy_n + 1)); fi
  sleep 1
done
R1="$(reqs)"; SECS=$(( $(date +%s) - A0 ))
RATES="$(jq -nc --argjson a "$R0" --argjson b "$R1" --argjson s "$SECS" '$b | to_entries | map({key, value: (((.value - ($a[.key] // 0)) * 60 / $s) | floor)}) | map(select(.value > 0)) | from_entries')"
TOTAL="$(jq '[.[]] | add // 0' <<<"$RATES")"
# Requests the activity causes, per reply: the log shipper's own messages left out. A reply names
# two paths, so a client that asks once per new path stays near 1.
PER_REPLY="$(jq -r '[to_entries[] | select(.key != "mac log") | .value] | add // 0 | . / 30 | . * 10 | floor / 10' <<<"$RATES")"
busy="$(awk -v s="$busy_sum" -v n="$busy_n" 'BEGIN { printf "%.1f", s / n }')"

TABLE="$(
  echo "perf chat: $SPACES spaces, each an agent pane of $REPLIES replies and a terminal"
  printf '%-36s %s\n' "seconds until CPU under 10%" "$settle"
  printf '%-36s %s\n' "idle CPU, mean of 10 s (%)" "$idle"
  printf '%-36s %s\n' "app CPU during the activity, mean (%)" "$busy"
  printf '%-36s %s\n' "app requests per minute, activity" "$TOTAL"
  printf '%-36s %s\n' "  by op" "$RATES"
  printf '%-36s %s\n' "app requests per reply (log left out)" "$PER_REPLY"
)"
echo "$TABLE"
if [ -n "${WHEEL_SHOTS:-}" ]; then mkdir -p "$WHEEL_SHOTS"; echo "$TABLE" >"$WHEEL_SHOTS/perf-chat.txt"; fi

check "the app settles under 10% CPU within 30 s of connecting" test "$settle" -le 30
check "idle CPU stays under 5%" awk -v c="$idle" 'BEGIN { exit !(c < 5) }'
check "typing and replies keep the app under 50% CPU" awk -v c="$busy" 'BEGIN { exit !(c < 50) }'
check "at most 3 requests per reply while an agent replies and a person types" awk -v c="$PER_REPLY" 'BEGIN { exit !(c <= 3) }'
finish
