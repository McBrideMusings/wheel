#!/usr/bin/env bash
# § Permissions artifact (wheel-j04): artifacts/permissions lists every request waiting on a person,
# from `wheel perm list` written by the artifact's refresh command. Drives the real Mac app against
# this script's own daemon with the fake agent: a pane shot of the artifact shows the request's row,
# and after `wheel perm answer` the next refresh shows "Nothing is waiting on you."
# Run it once per appearance (WHEEL_APPEARANCE=light|dark); WHEEL_SHOTS=<dir> keeps the PNGs.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
APPEARANCE="${WHEEL_APPEARANCE:-light}"
mkdir -p "$SHOTS"

use_fake_agent
printf '[artifacts.permissions]\npath = "%s/artifacts/permissions"\ncommand = %s\nevery = 1\n' \
  "$REPO" "'\"$WHEEL\" --url ws://127.0.0.1:PORT/ws --token-file $INST/token perm list > \"\$WHEEL_DATA_DIR/perms.json\"'" >>"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"
# The daemon picks its port after the entry is written: put the real one in the command.
sed -i '' "s/PORT/$PORT/" "$INST/wheel.toml"
prefs_reset

mkdir -p "$SCR/a"
A="$(w space new "$SCR/a")"
AGENT="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
w pane start "$AGENT" agent >/dev/null
W="$(w pane split "$AGENT" right)"
w pane kind "$W" web --artifact permissions --scope machine >/dev/null
bg w agent send "$AGENT" "permission please"
check "the agent's request is waiting" poll 20 bash -c "$(declare -f perm_id jget); WHEEL='$WHEEL' PORT=$PORT INST='$INST'; w() { \"\$WHEEL\" --url ws://127.0.0.1:\$PORT/ws --token-file \"\$INST/token\" \"\$@\"; }; [ -n \"\$(perm_id)\" ]"
PERM="$(perm_id)"

launch_app
check "the app connects" wait_st '.connection == "connected"' 20
w artifact refresh "$W" >/dev/null
check "the refresh wrote the request into the data file" grep -q "$PERM" "$INST/panes/$W/perms.json"
sleep 2
w pane shot "$W" "$SHOTS/permissions-waiting-$APPEARANCE.png"
echo "     shot $SHOTS/permissions-waiting-$APPEARANCE.png"

w perm answer "$PERM" allow >/dev/null
w artifact refresh "$W" >/dev/null
eq "after the answer the data file lists no request" "$(jq -r 'length' "$INST/panes/$W/perms.json")" "0"
sleep 2
w pane shot "$W" "$SHOTS/permissions-empty-$APPEARANCE.png"
echo "     shot $SHOTS/permissions-empty-$APPEARANCE.png"
finish
