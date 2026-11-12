#!/usr/bin/env bash
# § 8 Settings › General › Logs (wheel-5c6d.3): the push URL field and the shipping state beside it.
# Drives the real Mac app against this script's own daemon: off with no URL, failing with the
# error within 30 s of saving a closed port in the field, arriving once the real URL is saved,
# off again after the ✕. Saves a window shot of each state to $WHEEL_SHOTS (default $SCR/shots),
# named after WHEEL_APPEARANCE; run it once with WHEEL_APPEARANCE=light and once with dark.
#
# Needs LOKI_PUSH_URL.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

[ -n "${LOKI_PUSH_URL:-}" ] || { echo "set LOKI_PUSH_URL (~/.claude/.env)" >&2; exit 2; }
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
LOOK="${WHEEL_APPEARANCE:-system}"
mkdir -p "$SHOTS"

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
launch_app
check "the app connects" wait_st '.connection == "connected"' 20
app_post "/settings?action=open" >/dev/null
check "settings opens" wait_st '.settings.open and .settings.loaded' 10
eq "the General tab lists logs.push_url" "$(st '[.settings.rows[] | select(.key == "logs.push_url")] | length')" 1
check "with no URL the state is off" wait_st '.settings.logs.state == "off"' 10
eq "...and the row says so" "$(st .settings.logs.title)" "Off: the log, the usage journal and the apps’ lines stay on this Mac."
shot() { app_post "/settings?action=scroll&key=logs.push_url" >/dev/null; sleep 0.5; app_post "/window/snapshot?which=settings&path=$SHOTS/logs-$1-$LOOK.png" >/dev/null && echo "     shot $SHOTS/logs-$1-$LOOK.png"; }
shot off

# set_url <url>: type the URL into the General tab's field and press Return, as a person does.
set_url() {
  local q; q="$(jq -rn --arg t "$1" '$t|@uri')"
  app_post "/settings?action=edit&layer=global&key=logs.push_url&text=$q" >/dev/null
  app_post "/settings?action=save&layer=global&key=logs.push_url" >/dev/null
}

CLOSED="$(pick_port)"
set_url "http://127.0.0.1:$CLOSED/loki/api/v1/push"
T0=$SECONDS
check "a closed port shows failing within 30 s" wait_st '.settings.logs.state == "failing"' 30
echo "     failing after $((SECONDS - T0)) s"
eq "config get agrees with the field" "$(w config get logs.push_url)" "http://127.0.0.1:$CLOSED/loki/api/v1/push"
check "...with the connection error" wait_st '(.settings.logs.error // "") | length > 0' 1
echo "     error: $(st .settings.logs.error)"
check "...and bytes waiting" wait_st '.settings.logs.waiting_bytes > 0' 10
check "the row's title carries the error" wait_st '.settings.logs.title | startswith("Failing: ")' 1
check "...and its detail the bytes waiting" wait_st '.settings.logs.detail | endswith("waiting on this Mac.")' 1
shot failing

set_url "$LOKI_PUSH_URL"
check "the real URL shows ok within 30 s" wait_st '.settings.logs.state == "ok"' 30
check "...with the last success time" wait_st '.settings.logs.last_ok_ms > 0 and (.settings.logs.title | startswith("Arriving: last accepted "))' 1
check "...and no error" wait_st '.settings.logs.error == null' 1
shot ok

app_post "/settings?action=clear&layer=global&key=logs.push_url" >/dev/null
check "the ✕ turns it off" wait_st '.settings.logs.state == "off"' 15
eq "...and leaves no URL in the machine file" "$(w config get logs.push_url)" ""

MLOG="$(st .log_path)"
has() { grep -q -- "$1" "$2" 2>/dev/null; }
check "the app log has the failing state" has "log shipping: failing to http://127.0.0.1:$CLOSED/loki/api/v1/push" "$MLOG"
check "...and the ok state" has "log shipping: ok to " "$MLOG"
case "$LOKI_PUSH_URL" in
  *://*@*)
    CREDS="${LOKI_PUSH_URL#*://}"; CREDS="${CREDS%%@*}"
    check "the push URL's credentials are not in the app log" file_lacks "$CREDS" "$MLOG"
    ;;
esac
stop_daemon
check "with wheeld gone the window shows no shipping state" wait_st '.settings.logs == null' 15
app_post "/settings?action=close" >/dev/null
finish
