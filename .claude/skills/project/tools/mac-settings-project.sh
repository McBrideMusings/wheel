#!/usr/bin/env bash
# § 8 Settings › Project (wheel-lob2): the Project tab leaves out the keys only this machine's file
# sets. Drives the real Mac app against this script's own daemon with one space open: /state's
# settings rows carry machine_only, an edit of logs.push_url on the Project layer gets 404, and the
# General layer still edits it.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
SPACE="$(mkspaces 1)"
prefs_reset
launch_app
check "the app connects" wait_st '.connection == "connected"' 20
app_post "/settings?action=open" >/dev/null
check "settings opens" wait_st '.settings.open and .settings.loaded' 10
app_post "/settings?action=tab&tab=project" >/dev/null
app_post "/settings?action=project&id=$SPACE" >/dev/null
check "the rows are resolved for the space" wait_st ".settings.space == \"$SPACE\"" 10

eq "logs.push_url is marked machine-only" "$(st '.settings.rows[] | select(.key == "logs.push_url") | .machine_only')" true
eq "browser.homepage is not" "$(st '.settings.rows[] | select(.key == "browser.homepage") | .machine_only')" false
eq "every machine-only key is keybind or under one of the six prefixes" \
  "$(st '[.settings.rows[] | select(.machine_only) | .key | select(test("^((usage|agent|remote|status|logs|spaces)\\.|keybind$)") | not)] | length')" 0

code() { curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $(TOKEN)" "http://127.0.0.1:$DEBUG_PORT$1"; }
eq "the Project layer has no logs.push_url field (404)" "$(code '/settings?action=edit&layer=project&key=logs.push_url&text=x')" 404
eq "the Project layer edits browser.homepage (200)" "$(code '/settings?action=edit&layer=project&key=browser.homepage&text=about:blank')" 200
eq "the General layer still edits logs.push_url (200)" "$(code '/settings?action=edit&layer=global&key=logs.push_url&text=x')" 200
stop_daemon
