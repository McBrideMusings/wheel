#!/usr/bin/env bash
# § 8 The phone's own lines reach Loki: lines the app writes while wheeld is down wait in
# phone.log and go out once it is back, once each. Installs the built WheelPhone into a pool
# simulator, pairs it with this script's own daemon (logs.push_url = $LOKI_PUSH_URL), stops the
# daemon, relaunches the app with WHEEL_LOG_MARKS, starts the daemon again and asks Loki.
#
# Needs WHEEL_IOS_APP (the built Wheel.app for the simulator, WheelPhone scheme), LOKI_ADDR,
# LOKI_PUSH_URL and logcli.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SIM=~/.claude/tools/sim
BUNDLE=com.piercemakes.wheel.phone
UDID=""
ios_cleanup() { [ -n "$UDID" ] && "$SIM" release >/dev/null 2>&1; cleanup; }
trap ios_cleanup EXIT

[ -d "${WHEEL_IOS_APP:-}" ] || { echo "set WHEEL_IOS_APP to the built Wheel.app (WheelPhone, iphonesimulator)" >&2; exit 2; }
[ -n "${LOKI_PUSH_URL:-}" ] && [ -n "${LOKI_ADDR:-}" ] || { echo "set LOKI_ADDR and LOKI_PUSH_URL (~/.claude/.env)" >&2; exit 2; }

PORT="$(pick_port)"; RPORT="$(PORT=$((PORT + 1)); while ! port_free "$PORT"; do PORT=$((PORT + 1)); done; echo "$PORT")"
printf '[remote]\nlisten = "127.0.0.1:%s"\n[logs]\npush_url = "%s"\n' "$RPORT" "$LOKI_PUSH_URL" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"
mkdir -p "$SCR/a"
w space new "$SCR/a" >/dev/null

UDID="$("$SIM" acquire)"
xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
xcrun simctl install "$UDID" "$WHEEL_IOS_APP"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
PLOG="$DATA/Documents/phone.log"
plog() { grep -q -- "$1" "$PLOG" 2>/dev/null; }
marks() { [ "$(grep -c -- "log mark .* of 5 $TAG" "$PLOG" 2>/dev/null)" = 5 ]; }

SIMCTL_CHILD_WHEEL_PAIR_JSON="$(w remote pair --address 127.0.0.1)" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
check "the phone pairs" poll 40 plog "paired with wss://127.0.0.1:$RPORT/ws"

TAG="logship-$$-$RANDOM"
stop_daemon
xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1
SIMCTL_CHILD_WHEEL_LOG_MARKS="5:$TAG" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
check "five marks are written while wheeld is down" poll 20 marks
check "the app finds wheeld unreachable" poll 20 plog "daemon unreachable"
sleep 3
check "nothing reached wheeld while it was down" file_lacks "$TAG" "$INST/applog.jsonl"

start_daemon --state-file "$SCR/spaces.json"
check "the app reconnects" poll 30 plog "connected to wss://127.0.0.1:$RPORT/ws"
in_loki() {
  logcli query --quiet --since=10m --limit=100 --output=raw "{app=\"wheel\", source=\"app\", client=\"iphone\"} |= \"$TAG\"" 2>/dev/null >"$SCR/loki.txt"
  [ "$(wc -l <"$SCR/loki.txt" | tr -d ' ')" -ge 5 ]
}
check "the five marks reach Loki as client=iphone" poll 60 in_loki
# Long enough for a second shipping pass to have resent anything it would resend.
sleep 12
in_loki
for i in 1 2 3 4 5; do
  eq "mark $i is in Loki once" "$(grep -c -- "log mark $i of 5 $TAG" "$SCR/loki.txt")" 1
done
eq "wheeld kept each mark once" "$(grep -c -- "$TAG" "$INST/applog.jsonl")" 5
finish
