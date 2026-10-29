#!/usr/bin/env bash
# § Chat costs little at rest: the Mac app uses under 10% of one core and the daemon answers under
# 120 `file_peek` requests a minute while chat panes hold replies full of links.
#
#   idle-chat.sh          a scripted run: the Debug app against this script's own daemon, one agent
#                         pane with 30 replies (a file that exists, one that doesn't, a pane id, a
#                         web link), measured for $IDLE_WINDOW_S seconds while the agent keeps
#                         replying every 10 s.
#   idle-chat.sh --live   the installed app and the running wheeld, as they are, for the same
#                         window. Run it after every client deploy: real sessions stream far more
#                         than the fake agent does, and only they have ever set off a lookup flood.
IDLE_WINDOW_S="${IDLE_WINDOW_S:-60}"
WINDOW_S="$IDLE_WINDOW_S"

# cputime as M:SS.ss or H:MM:SS → seconds.
cpu_s() { ps -o time= -p "$1" | awk -F: '{ s = 0; for (i = 1; i <= NF; i++) s = s * 60 + $i; print s }'; }
peeks() { grep -c 'file_peek' "$1" 2>/dev/null || echo 0; }

# measure <app pid> <wheeld log>: prints "<% of one core> <file_peek per minute>".
measure() {
  local c0 c1 p0 p1
  c0="$(cpu_s "$1")"; p0="$(peeks "$2")"
  sleep "$WINDOW_S"
  c1="$(cpu_s "$1")"; p1="$(peeks "$2")"
  awk -v a="$c0" -v b="$c1" -v w="$WINDOW_S" -v p="$((p1 - p0))" \
    'BEGIN { printf "%.1f %d\n", (b - a) * 100 / w, p * 60 / w }'
}

if [ "${1:-}" = --live ]; then
  pid="$(pgrep -x Wheel | head -1)"
  [ -n "$pid" ] || { echo "no Wheel app running" >&2; exit 2; }
  log="$HOME/Library/Logs/wheel/wheeld.log"
  read -r pct per_min < <(measure "$pid" "$log")
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

REPLIES="${IDLE_REPLIES:-30}"

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset

mkdir -p "$SCR/a/docs"
echo "here" > "$SCR/a/docs/present.md"
A="$(w space new "$SCR/a")"
P="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
w pane start "$P" agent >/dev/null

launch_app
check "the app connects" wait_st '.connection == "connected"' 20
app_post "/focus?space=$A" >/dev/null
# One turn at a time: the agent takes a prompt only between turns. /state keeps the last 20 rows,
# so each wait looks for that turn's own reply.
for i in $(seq 1 "$REPLIES"); do
  w agent send "$P" "Turn $i: see docs/present.md:1, docs/missing-$i.md, pane $P and https://example.com/$i." >/dev/null
  wait_st ".panes[\"$P\"].transcript | any(startswith(\"agent: echo\") and contains(\"Turn $i:\"))" 20 || break
done
check "every reply arrives" wait_st ".panes[\"$P\"].transcript | any(startswith(\"agent: echo\") and contains(\"Turn $REPLIES:\"))" 5

# The session keeps going while it is measured, a short reply every 10 s: each change to it is a
# chance for the replies above to look their links up again. The wait is past the resolver's 15 s
# recheck, so a missing file is due again before the window opens.
app_post "/window/front" >/dev/null
sleep 20
check "the app looked the replies' paths up ($(peeks "$(LOG)") file_peek so far)" test "$(peeks "$(LOG)")" -gt 0
keep_talking() {
  local n=0
  while [ "$n" -lt $((WINDOW_S / 10)) ]; do n=$((n + 1)); w agent send "$P" "still here $n" >/dev/null; sleep 10; done
}
bg keep_talking
read -r pct per_min < <(measure "$(app_pid)" "$(LOG)")
echo "scripted: app ${pct}% of one core over ${WINDOW_S}s, ${per_min} file_peek/min"
check "the app uses under 10% of one core (${pct}%)" awk -v p="$pct" 'BEGIN { exit !(p < 10) }'
check "the daemon answers under 120 file_peek a minute (${per_min})" test "$per_min" -lt 120
finish
