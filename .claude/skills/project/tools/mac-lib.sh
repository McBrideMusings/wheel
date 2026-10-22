#!/usr/bin/env bash
# Helpers for the checks that drive the Mac app through its debug server. Source it after lib.sh
# and after start_daemon; never run it.
#
# The app runs from the build in $WHEEL_DERIVED_DATA (default $REPO/DerivedData) against this
# script's own daemon, with its own debug port and its own preferences domain, so the person's
# running Wheel, its windows and its saved layout are never touched. Needs jq.

command -v jq >/dev/null || { echo "the Mac checks need jq" >&2; exit 2; }
APP_DD="${WHEEL_DERIVED_DATA:-$REPO/DerivedData}"
APP="$APP_DD/Build/Products/Debug/Wheel.app"
[ -d "$APP" ] || { echo "missing $APP: run admin build mac first" >&2; exit 2; }
APP_BIN="$APP/Contents/MacOS/Wheel"
DEBUG_PORT=""
PREFS_SUITE="wheel.check.$$"
APP_HOME="$SCR/app-home"
mkdir -p "$APP_HOME"

# The app's PID: whoever listens on its debug port.
app_pid() { [ -n "$DEBUG_PORT" ] && lsof -nP -iTCP:"$DEBUG_PORT" -sTCP:LISTEN -t 2>/dev/null | head -1; }

# launch_app [VAR=value ...]: start the app in the background, without taking focus, and wait for its debug server.
launch_app() {
  [ -n "$DEBUG_PORT" ] || DEBUG_PORT="$(pick_port)"
  local extra=() v
  for v in "$@"; do extra+=(--env "$v"); done
  open -g -n \
    --env "WHEEL_URL=ws://127.0.0.1:$PORT/ws" \
    --env "WHEEL_TOKEN_FILE=$INST/token" \
    --env "WHEEL_DEBUG_PORT=$DEBUG_PORT" \
    --env "WHEEL_PREFS_SUITE=$PREFS_SUITE" \
    ${WHEEL_APPEARANCE:+--env "WHEEL_APPEARANCE=$WHEEL_APPEARANCE"} \
    "${extra[@]}" "$APP"
  poll 30 app_get /state
}

# app_get <path>: the debug server's reply, with the daemon's token as the debug server wants it.
app_get() { curl -sf -H "Authorization: Bearer $(TOKEN)" "http://127.0.0.1:$DEBUG_PORT$1"; }
app_post() { curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" "http://127.0.0.1:$DEBUG_PORT$1"; }
# st [jq options] <filter>: a value from the app's /state.
st() { app_get /state | jq -r "$@"; }

# wait_st <jq boolean expression> [seconds]: wait until /state satisfies the expression.
wait_st() {
  local i
  for i in $(seq 1 $((${2:-10} * 10))); do
    [ "$(st "$1" 2>/dev/null)" = true ] && return 0
    sleep 0.1
  done
  return 1
}

# quit_app: ⌘Q, then wait for the process to go.
quit_app() {
  local p; p="$(app_pid)"
  app_post /app/quit >/dev/null 2>&1
  [ -z "$p" ] || poll 15 sh -c "! kill -0 $p 2>/dev/null"
}

# kill_app: stop the app by PID, as the cleanup does.
kill_app() {
  local p; p="$(app_pid)"
  [ -z "$p" ] || { kill "$p" 2>/dev/null; poll 10 sh -c "! kill -0 $p 2>/dev/null"; }
}

# prefs_reset: forget the check's preferences domain.
prefs_reset() { defaults delete "$PREFS_SUITE" >/dev/null 2>&1; true; }

# mkspaces <n>: n project directories as spaces, printing their ids on stdout, one per line.
mkspaces() {
  local i d
  for i in $(seq 1 "$1"); do
    d="$SCR/proj$i"; mkdir -p "$d"
    w space new "$d"
  done
}

# win_ids: every window's id, in the app's order.
win_ids() { st '.windows[].id'; }
# win_of <space>: the id of the window listing the space, or nothing.
win_of() { st --arg s "$1" '.windows[] | select(.spaces | index($s)) | .id' 2>/dev/null; }

app_cleanup() { kill_app; defaults delete "$PREFS_SUITE" >/dev/null 2>&1; true; }
