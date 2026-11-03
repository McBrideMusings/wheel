#!/usr/bin/env bash
# § MCP Apps host (wheel-4bz4): a web pane showing an `mcp` entry draws the server's ui:// page in
# a sandboxed frame. The page's tool call reaches the server and its result reaches the page, a
# tool visible to the model only is refused, the frame cannot read the parent page or its
# `window.wheel`, and a server with no ui:// resource shows its tools. Needs `make build-client`,
# `cargo build --examples` and jq. With WHEEL_SHOTS set, keeps mcp-apps-<view|tools>.png.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

UI="$BIN/examples/mcp_ui_test_server"
PLAIN="$BIN/examples/mcp_test_server"
[ -x "$UI" ] && [ -x "$PLAIN" ] || { echo "missing the example MCP servers: run cargo build --examples" >&2; exit 2; }

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
PROJ="$SCR/proj"
mkdir -p "$PROJ"
printf '[artifacts.apps]\nmcp = "exec %s stdio"\n[artifacts.plain]\nmcp = "exec %s stdio"\n' "$UI" "$PLAIN" >"$PROJ/wheel.toml"
w space new "$PROJ" >/dev/null
launch_app
wait_st '.connection == "connected"' 20
SP="$(w space ls | jget spaces.0.id)"
P="$(w space ls | jget spaces.0.root.id)"
[ -z "${WHEEL_SHOTS:-}" ] || app_post "/window/front" >/dev/null
shot() { [ -z "${WHEEL_SHOTS:-}" ] || { sleep 2; w pane shot "$P" "$WHEEL_SHOTS/mcp-apps-$1.png" >/dev/null; }; }
reported() { w artifact call apps reported --space "$SP" 2>/dev/null; }

expect_exit 0 "a pane shows the MCP app artifact" w pane kind "$P" web --artifact apps
check "the view says its tool's result reached it, its button reached the server, and the model-only tool was refused" \
  poll 30 sh -c "$WHEEL --url ws://127.0.0.1:$PORT/ws --token-file $INST/token artifact call apps reported --space $SP 2>/dev/null | grep -q 'result:hello from show.*bump:.*bumped 1.*secret:refused'"
out="$(reported)"
echo "reported: $out"
case "$out" in *"escape:document:blocked,wheel:blocked,fetch:blocked"*) ok "the frame reaches neither the parent page, its wheel object nor the daemon" ;;
  *) fail "the frame's reach out of its sandbox" "$out" ;; esac
shot view

expect_exit 0 "a pane shows an MCP server with no ui:// resource" w pane kind "$P" web --artifact plain
check "the pane lists the server's tools" poll 20 sh -c "[ \"\$(curl -s -X POST -H 'Authorization: Bearer $(TOKEN)' -d '{\"pane\":\"$P\",\"js\":\"document.getElementById(\\\"tools\\\") ? document.getElementById(\\\"tools\\\").innerText : \\\"\\\"\"}' http://127.0.0.1:$DEBUG_PORT/web/eval | grep -c 'echo')\" = 1 ]"
shot tools
ev() { curl -s -X POST -H "Authorization: Bearer $(TOKEN)" -d "$(jq -n --arg p "$P" --arg js "$1" '{pane:$p,js:$js}')" "http://127.0.0.1:$DEBUG_PORT/web/eval"; }
has_text() { ev "var n=document.querySelector(\"$1\"); n ? n.innerText : \"\"" | grep -q "$2"; }
ev 'Array.from(document.querySelectorAll("#list button")).find(b=>b.textContent==="echo").click(); 1' >/dev/null
check "choosing echo shows a form with its text field" poll 10 has_text "#detail form" "text"
ev 'var i=document.querySelector("#detail input[type=text]"); i.value="typed in the form"; document.querySelector("#detail form").requestSubmit(); 1' >/dev/null
check "submitting the form shows the tool's result" poll 15 has_text "#detail .result" "typed in the form"
shot form
finish
