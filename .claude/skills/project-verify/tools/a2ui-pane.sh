#!/usr/bin/env bash
# § A2UI pane (wheel-x4kb): a web pane on an MCP server artifact whose tool links an `a2ui://`
# layout (A2UI over MCP) draws that layout as native controls. The example server
# (daemon/examples/mcp_a2ui_test_server.rs) serves a Notes surface with one component outside the
# catalog. Checks, against the Mac app's /state: the pane is native with the first data, the
# unsupported component is named, a button's action reaches the backend and changes /status and
# /count while /draft stays, and a plain MCP artifact stays a web page. Needs `make build-client`,
# `cargo build --examples` and jq. With WHEEL_SHOTS set, keeps a2ui-<pane|saved>.png.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

EX="$(dirname "$WHEEL")/examples"
[ -x "$EX/mcp_a2ui_test_server" ] || { echo "missing $EX/mcp_a2ui_test_server: run cargo build --examples" >&2; exit 2; }
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
PROJ="$SCR/proj"
mkdir -p "$PROJ"
printf '[artifacts.notes]\nmcp = "exec %s/mcp_a2ui_test_server stdio"\n[artifacts.plain]\nmcp = "exec %s/mcp_test_server stdio"\n' "$EX" "$EX" >"$PROJ/wheel.toml"
w space new "$PROJ" >/dev/null
launch_app
wait_st '.connection == "connected"' 20
P="$(w space ls | jget spaces.0.root.id)"
[ -z "${WHEEL_SHOTS:-}" ] || app_post "/window/front" >/dev/null
shot() { [ -z "${WHEEL_SHOTS:-}" ] || { sleep 2; w pane shot "$P" "$WHEEL_SHOTS/a2ui-$1.png" >/dev/null; }; }
pane_st() { app_get /state | jq -r --arg p "$P" ".panes[\$p] | $1"; }

expect_exit 0 "a pane shows the A2UI server's artifact" w pane kind "$P" web --artifact notes
check "the pane turns native" poll 40 sh -c "[ \"\$(curl -s -H 'Authorization: Bearer $(TOKEN)' http://127.0.0.1:$DEBUG_PORT/state | jq -r --arg p $P '(.panes[\$p]).a2ui.state')\" = ready ]"
eq "the first data is in the surface" "$(pane_st '.a2ui.data["/status"]')" "no notes yet"
eq "the draft is bound" "$(pane_st '.a2ui.data["/draft"]')" "Ship the spike"
eq "a component outside the catalog is named" "$(pane_st '.a2ui.unsupported | join(",")')" "Gizmo"
shot pane

app_post "/a2ui/action?pane=$P&name=save_note&text=Ship%20the%20spike" >/dev/null
check "the action reaches the backend and its update lands" poll 20 sh -c "[ \"\$(curl -s -H 'Authorization: Bearer $(TOKEN)' http://127.0.0.1:$DEBUG_PORT/state | jq -r --arg p $P '(.panes[\$p]).a2ui.data[\"/status\"]')\" = 'saved: Ship the spike' ]"
eq "only the changed paths changed: /count" "$(pane_st '.a2ui.data["/count"]')" "1 notes"
eq "only the changed paths changed: /draft" "$(pane_st '.a2ui.data["/draft"]')" "Ship the spike"
shot saved

expect_exit 0 "a pane shows a plain MCP artifact" w pane kind "$P" web --artifact plain
check "it stays a web page" poll 20 sh -c "[ \"\$(curl -s -H 'Authorization: Bearer $(TOKEN)' http://127.0.0.1:$DEBUG_PORT/state | jq -r --arg p $P '(.panes[\$p]).a2ui.state')\" = web ]"
finish
