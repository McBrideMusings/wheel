#!/usr/bin/env bash
# shots.sh [<surface>[:<state>] ...]: render every state of a Wheel surface in the real Mac app,
# light and dark, and print one PNG path per shot. No argument shoots every surface.
#
# Surfaces and states (DESIGN.md names each):
#   window : no-spaces, empty-pane, grid (a terminal beside an agent), sidebar-open
#   chat   : empty, finished, working, request
#
# Runs its own daemon and the Debug app from `make build-client` (project-verify's mac-lib.sh),
# never touching the person's Wheel. PNGs go to $WHEEL_SHOTS, default
# /private/tmp/claude/wheel/shots/<surface>-<state>-<theme>.png.
set -uo pipefail
VERIFY_TOOLS="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../project-verify/tools" && pwd)"
source "$VERIFY_TOOLS/lib.sh"
SHOTS="${WHEEL_SHOTS:-/private/tmp/claude/wheel/shots}"
mkdir -p "$SHOTS"

ALL="window:no-spaces window:empty-pane window:grid window:sidebar-open chat:empty chat:finished chat:working chat:request"
want=()
if [ $# -eq 0 ]; then want=($ALL); else
  for a in "$@"; do
    case "$a" in
      *:*) want+=("$a") ;;
      *) for s in $ALL; do [ "${s%%:*}" = "$a" ] && want+=("$s"); done ;;
    esac
  done
fi
[ ${#want[@]} -gt 0 ] || { echo "shots.sh: no such surface: $*" >&2; exit 2; }

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
source "$VERIFY_TOOLS/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT
prefs_reset

pane_of() { w space ls | jq -r --arg s "$1" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id'; }

# Each state builds its space on this script's own daemon and names the one to focus.
setup() {
  local state="$1" d s p p2
  d="$SCR/${state#*:}"; mkdir -p "$d"
  FOCUS=""
  case "$state" in
    window:no-spaces) ;;
    window:empty-pane) FOCUS="$(w space new "$d")" ;;
    window:grid|window:sidebar-open)
      s="$(w space new "$d")"; p="$(pane_of "$s")"
      w pane split "$p" right >/dev/null
      p2="$(w space ls | jq -r --arg s "$s" --arg p "$p" '[.spaces[] | select(.id==$s) | .root | .. | objects | select(.node=="pane") | .id | select(. != $p)] | first')"
      w pane start "$p" terminal >/dev/null
      w pane start "$p2" agent --message "hello" >/dev/null
      FOCUS="$s" ;;
    chat:empty)    s="$(w space new "$d")"; w pane start "$(pane_of "$s")" agent >/dev/null; FOCUS="$s" ;;
    chat:finished) s="$(w space new "$d")"; w pane start "$(pane_of "$s")" agent --message "hello" >/dev/null; FOCUS="$s" ;;
    chat:working)  s="$(w space new "$d")"; p="$(pane_of "$s")"; w pane start "$p" agent >/dev/null
                   bg w agent send "$p" "work slowly 600" >/dev/null 2>&1; FOCUS="$s" ;;
    chat:request)  s="$(w space new "$d")"; p="$(pane_of "$s")"; w pane start "$p" agent >/dev/null
                   bg w agent send "$p" "permission please" >/dev/null 2>&1; FOCUS="$s" ;;
  esac
}

for state in "${want[@]}"; do
  setup "$state"
  for theme in light dark; do
    WHEEL_APPEARANCE=$theme
    if [ -n "$(app_pid)" ]; then quit_app; fi
    launch_app
    wait_st '.connection == "connected"' 20 || { echo "shots.sh: the app did not connect" >&2; exit 1; }
    [ -n "$FOCUS" ] && app_post "/focus?space=$FOCUS" >/dev/null
    case "$state" in
      window:sidebar-open) app_post "/sidebar?open=true" >/dev/null ;;
      *) app_post "/sidebar?open=false" >/dev/null ;;
    esac
    app_post "/window/front" >/dev/null
    sleep 2
    out="$SHOTS/${state%%:*}-${state#*:}-$theme.png"
    app_post "/window/snapshot?path=$out" >/dev/null
    if [ -s "$out" ]; then echo "$out"; else echo "shots.sh: no snapshot for $state ($theme)" >&2; FAIL=$((FAIL + 1)); fi
  done
  unset WHEEL_APPEARANCE
  for s in $(w space ls | jq -r '.spaces[].id'); do w space close "$s" >/dev/null 2>&1 || true; done
done
exit $(( FAIL > 0 ))
