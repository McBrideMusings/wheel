#!/usr/bin/env bash
# § 3b The lead's MCP tools, over curl (and one scripted turn of the fake agent).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

STATE="$SCR/spaces.json"
PROJ="$SCR/proj"
mkdir -p "$PROJ"
start_daemon --lead-cmd "$FAKE_AGENT" --lead-cwd /tmp --state-file "$STATE"
lead_ready() { [ "$(w status | jget lead.phase)" = ready ]; }
poll 30 lead_ready || fail "the lead is ready" "$(w status | jget lead.phase)"

text() { mcp "$@" | mcp_text; }
iserr() { mcp "$@" | mcp_err; }
panes() { w pane ls | wc -l | tr -d ' '; }

# tools/list
LIST="$(curl -m 10 -s -X POST -H "Authorization: Bearer $(TOKEN)" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' "$(BASE)/mcp" -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}')"
for t in cancel_agent close_pane close_space create_bead create_space focus list_artifacts list_beads list_inbox list_spaces message_agent new_artifact open_artifact read_agent read_terminal set_inbox_item set_pane_kind show_bead split_pane start_pane type_into_pane update_bead wait_for_agent wait_for_text add_artifact remove_artifact; do
  if printf %s "$LIST" | grep -q "\"name\":\"$t\""; then ok "tools/list names $t"; else fail "tools/list names $t"; fi
done

# no space at all
eq "list_artifacts with no space is an error" "$(iserr list_artifacts '{}')" "true"
case "$(text list_artifacts '{}')" in *"no space to resolve an artifact against"*) ok "list_artifacts: no space to resolve an artifact against";; *) fail "list_artifacts text";; esac
case "$(text new_artifact '{"name":"dash"}')" in *"no space to resolve an artifact against"*) ok "new_artifact: no space to resolve an artifact against";; *) fail "new_artifact text";; esac
case "$(text open_artifact '{"name":"dash"}')" in *"no space to open an artifact in"*) ok "open_artifact: no space to open an artifact in";; *) fail "open_artifact text";; esac

# create_space asks the person; a missing directory is an error
R="$(mcp_asked allow create_space '{"cwd":"/nonexistent-dir-xyz"}')"
eq "create_space on a missing directory is an error" "$(printf %s "$R" | mcp_err)" "true"
R="$(mcp_asked allow create_space "{\"cwd\":\"$PROJ\"}")"
eq "create_space succeeds" "$(printf %s "$R" | mcp_err)" "false"
case "$R" in *'\"id\": \"s1\"'*|*'"id": "s1"'*) ok "...with id s1";; *) fail "create_space reply has id s1" "$R";; esac
eq "space ls shows the space" "$(w space ls | jget spaces.0.id)" "s1"
eq "...and it is active" "$(w space ls | jget active)" "s1"
eq "the first pane was made by the lead" "$(w pane ls --json | jget 0.created_by)" "lead"

# artifact tools
eq "list_artifacts {} is empty, not an error" "$(text list_artifacts '{}' | tr -d ' \n')" '{"artifacts":[],"resolves":{}}'
eq "new_artifact dash is not an error" "$(iserr new_artifact '{"name":"dash"}')" "false"
eq "...writes the folder holding only index.html" "$(ls -A "$PROJ/.wheel/artifacts/dash")" "index.html"
check "...and the entry in <cwd>/wheel.toml" grep -q '^\[artifacts.dash\]' "$PROJ/wheel.toml"
case "$(text new_artifact '{"name":"dash"}')" in *"there is already a project artifact dash"*) ok "the same name again is an error";; *) fail "the same name again";; esac
for bad in 'a/b' '..' '/etc' '.env' ''; do
  eq "new_artifact '$bad' is an error" "$(iserr new_artifact "{\"name\":\"$bad\"}")" "true"
done
check "no folder was left behind by the bad names" test "$(ls "$PROJ/.wheel/artifacts" | wc -l | tr -d ' ')" = 1
eq "new_artifact scope machine" "$(iserr new_artifact '{"name":"viewer","scope":"machine"}')" "false"
check "...lands in <inst>/artifacts/viewer" test -f "$INST/artifacts/viewer/index.html"
check "...with its entry in <inst>/wheel.toml" grep -q '^\[artifacts.viewer\]' "$INST/wheel.toml"
eq "list_artifacts reports project then machine" "$(text list_artifacts '{}' | jget artifacts.0.scope)" "project"
eq "...second is machine" "$(text list_artifacts '{}' | jget artifacts.1.scope)" "machine"
eq "new_artifact {pane: true} writes the flag" "$(iserr new_artifact '{"name":"flagged","pane":true}')" "false"
eq "list_artifacts {pane: true} filters to the flagged ones" "$(text list_artifacts '{"pane":true}' | jget artifacts.0.name)" "flagged"

# open_artifact
BEFORE="$(panes)"
R="$(mcp open_artifact '{"name":"dash"}')"
eq "open_artifact with no pane succeeds" "$(printf %s "$R" | mcp_err)" "false"
eq "...the space gains exactly one pane" "$(( $(panes) - BEFORE ))" "1"
eq "...shows.kind is web" "$(printf %s "$R" | mcp_text | jget shows.kind)" "web"
eq "...shows.scope is project" "$(printf %s "$R" | mcp_text | jget shows.scope)" "project"
eq "...web_panes carries the resolved folder" "$(w space ls | jget web_panes.0.dir)" "$PROJ/.wheel/artifacts/dash"
BEFORE="$(panes)"
for args in '{"name":"nosuch"}' '{"name":".."}' '{"name":"a/b"}' '{"name":""}' '{"name":"dash","scope":"hologram"}' '{"name":"dash","pane":"p999"}'; do
  eq "open_artifact $args is an error" "$(iserr open_artifact "$args")" "true"
done
eq "the pane count is unchanged after every bad open" "$(panes)" "$BEFORE"
has_log "artifact opened pane=.* name=dash .*split=true" "log: artifact opened ... split=true"
has_log "artifact created" "log: artifact created"

# split_pane, set_pane_kind, focus, close_pane, close_space
FIRST="$(w pane ls | awk '{print $2}' | head -1)"
FOCUS_BEFORE="$(w space ls | jget spaces.0.focus)"
R="$(mcp split_pane "{\"pane\":\"$FIRST\",\"dir\":\"right\"}")"
eq "split_pane succeeds" "$(printf %s "$R" | mcp_err)" "false"
eq "...and leaves the focus where it was" "$(w space ls | jget spaces.0.focus)" "$FOCUS_BEFORE"
SPLITPANE="$(w pane ls | awk '{print $2}' | tail -1)"
case "$(text split_pane "{\"pane\":\"$FIRST\",\"dir\":\"diagonal\"}")" in *left*right*up*down*|*"left, right, up, down"*) ok "dir diagonal names left, right, up, down";; *) fail "dir diagonal text";; esac
eq "dir diagonal is an error" "$(iserr split_pane "{\"pane\":\"$FIRST\",\"dir\":\"diagonal\"}")" "true"
NEW="$(w pane ls --json | grep -B4 '"kind": "empty"' | grep '"pane"' | tail -1 | sed 's/.*"pane": "\(.*\)".*/\1/')"
check "a pane to change the kind of exists" test -n "$NEW"
eq "set_pane_kind terminal succeeds" "$(iserr set_pane_kind "{\"pane\":\"$NEW\",\"kind\":\"terminal\"}")" "false"
check "list_spaces then shows that pane with a term" bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' space ls | grep -q '\"term\"'"
eq "set_pane_kind hologram is an error" "$(iserr set_pane_kind "{\"pane\":\"$NEW\",\"kind\":\"hologram\"}")" "true"
eq "set_pane_kind agent is an error" "$(iserr set_pane_kind "{\"pane\":\"$NEW\",\"kind\":\"agent\"}")" "true"
eq "focus succeeds" "$(iserr focus "{\"pane\":\"$FIRST\"}")" "false"
case "$(text close_pane '{"pane":"p999"}')" in *"no pane p999"*) ok "close_pane p999: no pane p999";; *) fail "close_pane p999 text";; esac
eq "close_pane p999 is an error" "$(iserr close_pane '{"pane":"p999"}')" "true"

# auth and origin
eq "a wrong bearer gets 401" "$(curl -m 10 -s -o /dev/null -w '%{http_code}' -X POST -H 'Authorization: Bearer wrong' -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' "$(BASE)/mcp" -d '{}')" "401"
eq "an Origin header gets 403" "$(curl -m 10 -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $(TOKEN)" -H 'Origin: http://evil.example' -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' "$(BASE)/mcp" -d '{}')" "403"
has_log 'refused: missing or wrong token via="mcp POST /mcp"' "log: refused wrong token via mcp"
has_log 'refused: browser origin via="mcp POST /mcp"' "log: refused browser origin via mcp"
has_log 'mcp tool ok tool="create_space"' "log: mcp tool ok create_space"
has_log 'mcp tool failed' "log: mcp tool failed"
has_log 'session spawned session=lead.*mcp=http://127.0.0.1:' "log: lead spawned with the mcp url"

# type_into_pane: a terminal the lead made types at once; one the person made asks
TP="$NEW"
OUT="$(mcp type_into_pane "{\"pane\":\"$TP\",\"text\":\"echo typed-\$((6*7))\",\"enter\":true}")"
eq "type_into_pane on the lead's own terminal types with no request" "$(printf %s "$OUT" | mcp_err)" "false"
TERM_ID="$(w space ls | grep -A6 "\"id\": \"$TP\"" | grep -m1 '"term"' | sed 's/.*"term": "\(.*\)".*/\1/')"
expect_exit 0 "...and the shell prints typed-42" w term read "$TERM_ID" --until typed-42 --timeout 20
eq "type_into_pane on an empty pane is an error" "$(iserr type_into_pane "{\"pane\":\"$FIRST\",\"text\":\"x\"}")" "true"

# a scripted turn of the fake agent through the lead
R="$(w lead send hello 2>&1)"
case "$R" in *"echo: hello"*) ok "wheel lead send hello prints echo: hello";; *) fail "lead send hello" "$R";; esac

(w lead send "make a space" >"$SCR/send.out" 2>&1) & SP=$!; BG_PIDS+=("$SP")
poll 15 perm_id
case "$(w perm list | jget 0.title)" in 'Open a space for '*) ok "the lead's create_space raises 'Open a space for ...'";; *) fail "create_space request title" "$(w perm list | jget 0.title)";; esac
w perm answer "$(perm_id)" allow >/dev/null 2>&1
wait "$SP" 2>/dev/null
check "wheel lead send 'make a space' prints the new space as mcp: {...}" grep -q '^mcp: {' "$SCR/send.out"
check "...and the fake agent's echo" grep -q '^echo: make a space' "$SCR/send.out"

finish
