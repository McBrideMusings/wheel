#!/usr/bin/env bash
# § 2f Process list: `wheel space ps` and `wheel space kill` over a terminal pane's process tree; and,
# with the Mac app built, the app's process card listing the same rows (POST /procs, /state procs).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

PROJ="$SCR/proj"; mkdir -p "$PROJ"
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
S="$(w space new "$PROJ")"
P="$(w space ls | jget spaces.0.root.id)"
w pane kind "$P" terminal >/dev/null
T="$(w space ls | jget spaces.0.root.term)"
w term send "$T" 'sleep 4243' --enter

# listed <n>: the space has n `sleep 4243` rows in the CLI's list.
listed() { [ "$(w space ps "$S" --json | jq '[.[] | select(.command=="sleep 4243")] | length')" = "$1" ]; }
row() { w space ps "$S" --json | jq -r --arg p "$P" '[.[] | select(.command=="sleep 4243" and .pane==$p)][0] | '"$1"; }
poll 20 listed 1
PID="$(row .pid)"
check "space ps lists the sleep under its pane" test -n "$PID" -a "$PID" != null
eq "the row names the program" "$(row .name)" "sleep"
check "the text form lists it too" sh -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' space ps '$S' | grep -q 'sleep 4243'"
check "space kill refuses a pid outside the space" sh -c "! '$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' space kill '$S' 1"

if [ -d "${WHEEL_DERIVED_DATA:-$CHECKS_DIR/../../../../DerivedData}/Build/Products/Debug/Wheel.app" ]; then
  launch_app
  wait_st '.windows | length == 1' 20
  app_post "/procs?space=$S&action=list" >/dev/null
  check "the Mac app's process card lists the same sleep" wait_st ".procs.\"$S\" | map(select(.command==\"sleep 4243\")) | length == 1" 20
  app_post "/procs?space=$S&action=stop&pid=$PID" >/dev/null
  check "a stop from the app removes it from the app's list" wait_st ".procs.\"$S\" | map(select(.command==\"sleep 4243\")) | length == 0" 20
  check "...and from the CLI's" poll 20 listed 0
else
  echo "skipped the Mac app half (no build)"
  expect_exit 0 "space kill stops it" w space kill "$S" "$PID"
  check "the stopped process leaves the list" poll 20 listed 0
fi
has_log "process stopped" "log: process stopped"
finish
