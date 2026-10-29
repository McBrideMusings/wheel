#!/usr/bin/env bash
# § 2c-builtins in the Mac app (wheel-9gxt.5): a diff post is drawn by the `diff` built-in, and a
# question post is answered through the page by `POST /web/eval` (the debug route that runs a script
# in a web pane's page). Needs `admin build mac`. With WHEEL_SHOTS set, the window snapshot saves each
# state to it, named after WHEEL_APPEARANCE: run it with light, then dark.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

start_daemon --state-file "$SCR/spaces.json"
prefs_reset
LOOK="${WHEEL_APPEARANCE:-system}"
mkdir -p "$SCR/proj"
w space new "$SCR/proj" >/dev/null
launch_app
wait_st '.connection == "connected"' 20
P="$(w space ls | jget spaces.0.root.id)"
# WebKit stops painting a covered window and is slow to resume, so the window comes forward once.
[ -z "${WHEEL_SHOTS:-}" ] || app_post "/window/front" >/dev/null
shot() { [ -z "${WHEEL_SHOTS:-}" ] || { sleep 2; app_post "/window/snapshot?path=$WHEEL_SHOTS/$2-$LOOK.png" >/dev/null; }; }
# eval <pane> <js>: the page's value for the script, as JSON.
eval_js() { curl -s -X POST -H "Authorization: Bearer $(TOKEN)" -d "$(jq -cn --arg pane "$1" --arg js "$2" '{pane: $pane, js: $js}')" "http://127.0.0.1:$DEBUG_PORT/web/eval"; }

# The diff post.
expect_exit 0 "the diff built-in installs" w artifact builtin install diff
cat >"$SCR/change.patch" <<'PATCH'
diff --git a/src/a.txt b/src/a.txt
--- a/src/a.txt
+++ b/src/a.txt
@@ -1,3 +1,3 @@
 keep
-old-line
+new-line
 tail
PATCH
expect_exit 0 "a diff post is made with --as" w artifact new change --as diff "$SCR/change.patch"
expect_exit 0 "a pane shows it" w pane kind "$P" web --artifact change
check "the page draws the added and removed rows" poll 20 sh -c "[ \"\$(curl -s -X POST -H 'Authorization: Bearer $(TOKEN)' -d '{\"pane\":\"$P\",\"js\":\"document.querySelectorAll(\\\".row.add\\\").length + \\\",\\\" + document.querySelectorAll(\\\".row.del\\\").length\"}' http://127.0.0.1:$DEBUG_PORT/web/eval)\" = '{\"result\":\"1,1\"}' ]"
eq "the file header names the file" "$(eval_js "$P" 'document.querySelector("h2 span").textContent' | jq -r .result)" "src/a.txt"
shot "$P" diff

# The question post, an ordinary machine artifact.
expect_exit 0 "the question page registers at machine scope" w artifact add "$REPO/artifacts/question" --name question --scope machine
printf '{"prompt":"Ship it?","options":["yes","no"]}' >"$SCR/question.json"
expect_exit 0 "a question with options is posted" w artifact new q1 --as question "$SCR/question.json"
Q="$(w pane split "$P" right)"
expect_exit 0 "a pane shows it" w pane kind "$Q" web --artifact q1
check "the page draws one button per option" poll 20 sh -c "[ \"\$(curl -s -X POST -H 'Authorization: Bearer $(TOKEN)' -d '{\"pane\":\"$Q\",\"js\":\"document.querySelectorAll(\\\"#options button\\\").length\"}' http://127.0.0.1:$DEBUG_PORT/web/eval)\" = '{\"result\":2}' ]"
shot "$Q" question-options
eval_js "$Q" 'document.querySelector("#options button").click(); document.getElementById("send").click(); "clicked"' >/dev/null
eq "the answer reaches artifact state --wait" "$(w artifact state "$Q" answer --wait | jq -c .)" '{"choice":"yes"}'

printf '{"prompt":"What is the port?"}' >"$SCR/free.json"
cp "$SCR/free.json" "$SCR/question.json"
expect_exit 0 "a question with no options is posted" w artifact new q2 --as question "$SCR/question.json"
R="$(w pane split "$Q" down)"
expect_exit 0 "a pane shows it" w pane kind "$R" web --artifact q2
check "the page draws a text box" poll 20 sh -c "[ \"\$(curl -s -X POST -H 'Authorization: Bearer $(TOKEN)' -d '{\"pane\":\"$R\",\"js\":\"String(!document.getElementById(\\\"text\\\").hidden)\"}' http://127.0.0.1:$DEBUG_PORT/web/eval)\" = '{\"result\":\"true\"}' ]"
shot "$R" question-text
eval_js "$R" 'const t = document.getElementById("text"); t.value = "port 7420"; t.dispatchEvent(new Event("input")); document.getElementById("send").click(); "typed"' >/dev/null
eq "typed text reaches artifact state --wait" "$(w artifact state "$R" answer --wait | jq -c .)" '{"text":"port 7420"}'

check "a pane that is not a web pane is refused" sh -c "curl -s -X POST -H 'Authorization: Bearer $(TOKEN)' -d '{\"pane\":\"p999\",\"js\":\"1\"}' http://127.0.0.1:$DEBUG_PORT/web/eval | grep -q 'shows no artifact page'"
check "the app log has the eval" grep -q "debug: web eval on $Q" "$(st .log_path)"
finish
