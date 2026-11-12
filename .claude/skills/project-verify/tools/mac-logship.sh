#!/usr/bin/env bash
# § 8 The Mac app's own lines reach Loki: the app writes its log beside the daemon it uses, sends
# its lines to wheeld while connected, and lines written while wheeld is down go out once it is
# back, once each. Drives the real Mac app against this script's own daemon
# (logs.push_url = $LOKI_PUSH_URL), relaunches it with WHEEL_LOG_MARKS while the daemon is stopped,
# starts the daemon again and asks Loki.
#
# Needs LOKI_ADDR, LOKI_PUSH_URL and logcli.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

[ -n "${LOKI_PUSH_URL:-}" ] && [ -n "${LOKI_ADDR:-}" ] || { echo "set LOKI_ADDR and LOKI_PUSH_URL (~/.claude/.env)" >&2; exit 2; }

printf '[logs]\npush_url = "%s"\n' "$LOKI_PUSH_URL" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
launch_app
check "the app connects" wait_st '.connection == "connected"' 20
MLOG="$INST/WheelMac.log"
eq "the app's log is beside its daemon's token" "$(st .log_path)" "$MLOG"
has() { grep -q -- "$1" "$2" 2>/dev/null; }
check "wheeld keeps the app's lines as client mac" poll 10 has '"client":"mac"' "$INST/applog.jsonl"

TAG="mac-logship-$$-$RANDOM"
kill_app
stop_daemon
launch_app "WHEEL_LOG_MARKS=5:$TAG"
marks() { [ "$(grep -c -- "log mark .* of 5 $TAG" "$MLOG" 2>/dev/null)" = 5 ]; }
check "five marks are written while wheeld is down" poll 20 marks
check "the app finds wheeld unreachable" poll 20 has "daemon unreachable" "$MLOG"
sleep 3
check "nothing reached wheeld while it was down" file_lacks "$TAG" "$INST/applog.jsonl"

start_daemon --state-file "$SCR/spaces.json"
check "the app reconnects" wait_st '.connection == "connected"' 30
in_loki() {
  logcli query --quiet --since=10m --limit=100 --output=raw "{app=\"wheel\", source=\"app\", client=\"mac\"} |= \"$TAG\"" 2>/dev/null >"$SCR/loki.txt"
  [ "$(wc -l <"$SCR/loki.txt" | tr -d ' ')" -ge 5 ]
}
check "the five marks reach Loki as client=mac" poll 60 in_loki
# Long enough for a second shipping pass to have resent anything it would resend.
sleep 12
in_loki
for i in 1 2 3 4 5; do
  eq "mark $i is in Loki once" "$(grep -c -- "log mark $i of 5 $TAG" "$SCR/loki.txt")" 1
done
eq "wheeld kept each mark once" "$(grep -c -- "$TAG" "$INST/applog.jsonl")" 5
finish
