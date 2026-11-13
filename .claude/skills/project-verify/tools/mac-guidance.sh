#!/usr/bin/env bash
# Settings › Guidance in the Mac app (wheel-bxxa): the tab lists Wheel's text read-only, the file for
# every agent and one per agent; Open in Editor makes a missing file; an older server shows only
# "The server was updated.". Window shots of each state go to $WHEEL_SHOTS (when set), named after
# WHEEL_APPEARANCE: run it with light, then dark.
#   OLD_WHEELD=<a wheeld from before the guidance ops>  also drives the older-server state.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

LOOK="${WHEEL_APPEARANCE:-system}"
shot() { [ -z "${WHEEL_SHOTS:-}" ] || { app_post "/window/front?which=settings" >/dev/null; sleep 0.8; app_post "/window/snapshot?which=settings&path=$WHEEL_SHOTS/guidance-$1-$LOOK.png" >/dev/null; }; }
tab() { app_post "/settings?action=tab&tab=$1" >/dev/null; }

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
launch_app
wait_st '.connection == "connected"' 20
app_post "/settings?action=open" >/dev/null
tab guidance
app_post "/settings?action=resize&width=720&height=1150" >/dev/null || true

check "the tab loads the server's guidance" wait_st '.settings.guidance.loaded' 10
eq "it lists the file for every agent and one per agent" "$(st '.settings.guidance.files | map(.agent) | join(",")')" ",claude,codex,pi"
eq "every file is empty on a fresh install" "$(st '[.settings.guidance.files[] | select(.text != "")] | length')" "0"
check "Wheel's own text is there" wait_st '.settings.guidance.brief_bytes > 1000' 5
check "the all-agents file lives beside wheel.toml" sh -c "[ \"$(st '.settings.guidance.files[0].path')\" = \"$INST/guidance.md\" ]"
shot empty

mkdir -p "$INST/guidance"
printf 'Put a rendered page in a pane beside each chat.\n' >"$INST/guidance.md"
printf 'Codex: keep answers short.\n' >"$INST/guidance/codex.md"
tab general
tab guidance
check "an edit made elsewhere shows when the tab opens again" wait_st '.settings.guidance.files[0].text | test("rendered page")' 10
eq "the per-agent file shows under its agent" "$(st '.settings.guidance.files[] | select(.agent=="codex") | .text')" "Codex: keep answers short."
eq "the right column cuts the text per document" "$(st '.settings.guidance.segments | map(.id) | join(",")')" "wheel,all,claude"
check "Wheel's block is the brief, the all-agents block carries its heading and text" wait_st '.settings.guidance.segments[1].text | test("every agent:\n.*rendered page")' 5
eq "claude has no file, so its block is empty" "$(st '.settings.guidance.segments[2].text')" ""
eq "the blocks join into the text the server gave" "$(st '.settings.guidance.segments | map(.text) | join("")')" "$(st '.settings.guidance.given')"
app_post "/settings?action=guidance_focus&id=all" >/dev/null
sleep 0.5
shot files
app_post "/settings?action=guidance_preview&agent=codex" >/dev/null
check "choosing codex shows codex's text after the all-agents text" wait_st '.settings.guidance.preview_agent == "codex" and (.settings.guidance.segments[2].text | test("for codex:\n.*keep answers short"))' 10
eq "and the blocks still join into the server's text" "$(st '.settings.guidance.segments | map(.text) | join("")')" "$(st '.settings.guidance.given')"
app_post "/settings?action=guidance_focus&id=codex" >/dev/null
shot codex
app_post "/settings?action=guidance_preview&agent=claude" >/dev/null

check "Open in Editor makes a missing file" test ! -e "$INST/guidance/pi.md"
app_post "/settings?action=guidance_open&agent=pi" >/dev/null
check "the server made it" wait_st '.settings.guidance.opened_file.path | endswith("guidance/pi.md")' 10
check "and it exists, empty" sh -c "[ -f '$INST/guidance/pi.md' ] && [ ! -s '$INST/guidance/pi.md' ]"
eq "a file with text is kept" "$(cat "$INST/guidance/codex.md")" "Codex: keep answers short."

APPLOG="$(st .log_path)"
check "the app log has the listing" grep -q "guidance: 4 files listed, 2 with text" "$APPLOG"
check "and the open" grep -q "guidance: open the file for pi" "$APPLOG"

if [ -n "${OLD_WHEELD:-}" ]; then
  app_post "/settings?action=close" >/dev/null
  app_cleanup
  stop_daemon
  WHEELD="$OLD_WHEELD"
  start_daemon --state-file "$SCR/spaces2.json"
  prefs_reset
  launch_app
  wait_st '.connection == "connected"' 20
  app_post "/settings?action=open" >/dev/null
  tab guidance
  sleep 1
  eq "an older server leaves the tab without guidance" "$(st '.settings.guidance.loaded')" "false"
  shot older
fi
finish
