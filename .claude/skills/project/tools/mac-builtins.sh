#!/usr/bin/env bash
# § 2c-builtins in the Mac app (wheel-9gxt.2): Settings › Artifacts lists the built-ins, installs,
# stops an update on an edited file with its diff, and removes. Window shots of the tab before and
# after Install go to $WHEEL_SHOTS (when set), named after WHEEL_APPEARANCE: run it with light, then dark.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
DIR="$INST/artifacts/sample"
LOOK="${WHEEL_APPEARANCE:-system}"
shot() { [ -z "${WHEEL_SHOTS:-}" ] || { app_post "/window/front?which=settings" >/dev/null; sleep 0.6; app_post "/window/snapshot?which=settings&path=$WHEEL_SHOTS/artifacts-$1-$LOOK.png" >/dev/null; }; }

launch_app
wait_st '.connection == "connected"' 20
app_post "/settings?action=open" >/dev/null
app_post "/settings?action=tab&tab=artifacts" >/dev/null
check "the tab lists the built-ins" wait_st '.builtins.loaded and (.builtins.rows | length) >= 1' 10
eq "sample offers Install" "$(st '.builtins.rows[] | select(.name=="sample") | .action')" "install"
shot before

app_post "/builtins?action=install&name=sample" >/dev/null
check "Install installs it" wait_st '.builtins.rows[] | select(.name=="sample") | .installed_version == 1' 10
eq "and it is up to date" "$(st '.builtins.rows[] | select(.name=="sample") | .action')" "up_to_date"
check "the page is in the machine artifacts folder" test -f "$DIR/index.html"
expect_out '^sample  shipped 1  installed 1$' "the CLI agrees" w artifact builtin ls
sleep 0.5
shot after

printf '<p>mine</p>\n' >"$DIR/index.html"
app_post "/builtins?action=ls" >/dev/null
check "the row reports the edit" wait_st '.builtins.rows[] | select(.name=="sample") | .edited_files == ["index.html"]' 10
app_post "/builtins?action=update&name=sample" >/dev/null
check "Update stops on the edited file" wait_st '.builtins.edited.name == "sample"' 10
check "with its diff" sh -c "printf %s \"\$(curl -sf -H 'Authorization: Bearer $(TOKEN)' http://127.0.0.1:$DEBUG_PORT/state | jq -r '.builtins.edited.files[0].diff')\" | grep -q '^-<p>mine</p>'"
eq "and changes nothing" "$(cat "$DIR/index.html")" "<p>mine</p>"
sleep 0.5
[ -z "${WHEEL_SHOTS:-}" ] || app_post "/window/snapshot?which=sheet&path=$WHEEL_SHOTS/artifacts-edited-$LOOK.png" >/dev/null
app_post "/builtins?action=edited&choice=replace" >/dev/null
check "Replace puts the shipped page back" wait_st '.builtins.rows[] | select(.name=="sample") | .edited_files == []' 10
check "on disk" grep -q 'Sample built-in' "$DIR/index.html"

app_post "/builtins?action=remove&name=sample" >/dev/null
check "Remove uninstalls it" wait_st '.builtins.rows[] | select(.name=="sample") | .action == "install"' 10
check "and the folder is gone" test ! -e "$DIR"

mkdir -p "$DIR"
app_post "/builtins?action=install&name=sample" >/dev/null
check "a refused install shows why" wait_st '.builtins.problem // "" | test("Wheel did not install it")' 10

APPLOG="$(st .log_path)"
check "the app log has the install" grep -q "built-in install sample" "$APPLOG"
check "and the refusal" grep -q "built-in sample refused" "$APPLOG"
finish
