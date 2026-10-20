#!/usr/bin/env bash
# § 3d The lead's inbox (ADR-0008): items from agent panes, the lead's prefix, closing, MCP tools.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

STATE="$SCR/spaces.json"
PROJ="$SCR/proj"
mkdir -p "$PROJ"
start_daemon --lead-cmd "$FAKE_AGENT" --lead-cwd /tmp --state-file "$STATE" --orchestration
lead_ready() { [ "$(w status | jget lead.phase)" = ready ]; }
poll 30 lead_ready || fail "the lead is ready" "$(w status | jget lead.phase)"

# item <id> <field>: a field of one inbox item, by id.
item() { w inbox list | tr -d '\n ' | sed 's/},{/}\n{/g' | grep "\"id\":\"$1\"" | sed "s/.*\"$2\":\"\{0,1\}\([^\",}]*\).*/\1/"; }
nitems() { w inbox list | jget .; }

w space new "$PROJ" --name web >/dev/null
P="$(w pane ls | awk '{print $2}')"
R="$(w pane start "$P" agent --message "first task")"
SESSION="$(printf %s "$R" | jget session)"
check "pane start agent prints a session" test -n "$SESSION"
turn_ended() { [ "$(item i1 kind)" = turn_ended ]; }
poll 20 turn_ended
eq "the first item is a turn_ended" "$(item i1 kind)" "turn_ended"
eq "...state new" "$(item i1 state)" "new"
eq "...detail end_turn" "$(item i1 detail)" "end_turn"
eq "...source names the pane" "$(w inbox list | jget 0.source.pane)" "$P"
eq "...and the session" "$(w inbox list | jget 0.source.session)" "$SESSION"
eq "...and the space" "$(w inbox list | jget 0.source.space)" "s1"
eq "status carries inbox_new: 1" "$(w status | jget inbox_new)" "1"
eq "inbox.json holds the same item" "$(jget items.0.kind <"$INST/inbox.json")" "turn_ended"
has_log "inbox item added item=i1 kind=TurnEnded" "log: inbox item added"
has_log "inbox count new=1 open=1" "log: inbox count new=1 open=1"

# a restart keeps the item; the plain fake agent cannot resume the pane, so a failed item joins it
restart_daemon
poll 30 lead_ready
eq "after a restart i1 is listed unchanged" "$(item i1 state)" "new"
failed() { [ "$(item i2 kind)" = failed ]; }
poll 20 failed
eq "...beside an i2 failed item" "$(item i2 kind)" "failed"
case "$(w inbox list | jget 1.detail)" in *"this agent cannot resume"*) ok "...whose detail is: session ended: this agent cannot resume";; *) fail "i2 detail" "$(w inbox list | jget 1.detail)";; esac
has_log "inbox loaded.*items=1" "log: inbox loaded items=1"

# the lead's next message carries a prefix and marks the items seen
N="$(w status | jget inbox_new)"
R="$(w lead send "hello lead" 2>&1)"
case "$R" in "echo: [inbox: $N new — web $N]"*) ok "lead send echoes the inbox prefix for $N new";; *) fail "lead send prefix" "$R";; esac
case "$R" in *"hello lead"*) ok "...then the message";; *) fail "...then the message" "$R";; esac
eq "the items are now seen" "$(item i1 state)" "seen"
has_log "inbox prefix added" "log: inbox prefix added"
has_log "inbox items seen" "log: inbox items seen"
has_log "inbox count new=0" "log: inbox count new=0"
R="$(w lead send "again" 2>&1)"
case "$R" in "echo: again"*) ok "the next lead send has no prefix";; *) fail "next send has no prefix" "$R";; esac

# an agent message closes the pane's open items and adds its own turn_ended
AGENT_PANE="$(w space ls | grep -B2 -A6 '"kind": "agent"' | grep -m1 '"id": "p' | sed 's/.*"id": "\(.*\)".*/\1/')"
check "an agent pane exists" test -n "$AGENT_PANE"
w agent send "$SESSION" "again" >/dev/null 2>&1
closed_i1() { [ "$(item i1 state)" = closed ]; }
poll 15 closed_i1
eq "agent send closes the pane's open items" "$(item i1 state)" "closed"
has_log 'inbox item closed.*why="pane got a message"' "log: closed why=pane got a message"
new_turn() { [ -n "$(item i3 kind)" ] || [ -n "$(item i4 kind)" ]; }
poll 15 new_turn
check "...and its turn end adds a new item" new_turn

# a message with "permission" adds a permission item that closes when answered
(w agent send "$SESSION" "needs permission" >"$SCR/perm.out" 2>&1) & SP=$!; BG_PIDS+=("$SP")
poll 15 perm_id
PERM="$(perm_id)"
check "a permission request is waiting" test -n "$PERM"
perm_item() { w inbox list | tr -d '\n ' | grep -o '"id":"i[0-9]*"[^}]*}[^}]*"kind":"permission"[^}]*"state":"[a-z]*"' | head -1; }
check "a permission item is in the inbox" test -n "$(perm_item)"
w perm answer "$PERM" allow >/dev/null 2>&1
wait "$SP" 2>/dev/null
has_log 'inbox item closed' "log: an item closed after the permission answer"
case "$(w inbox list | tr -d '\n ' | grep -o '"kind":"permission"[^}]*"state":"[a-z]*"' | tail -1)" in *'"state":"closed"'*) ok "the permission item closes when answered";; *) fail "permission item closes" "$(w inbox list | tr -d '\n ' | grep -o '"kind":"permission"[^}]*"state":"[a-z]*"')";; esac

# "die now" adds a failed item
w agent send "$SESSION" "die now" >/dev/null 2>&1
died() { [ "$(w inbox list | tr -d '\n ' | grep -o '"kind":"failed"' | wc -l | tr -d ' ')" -ge 2 ]; }
poll 15 died
check "a message with 'die now' adds a failed item" died

# inbox close, and the MCP tools
expect_exit 0 "inbox close i2 exits 0" w inbox close i2
eq "...the item is closed" "$(item i2 state)" "closed"
expect_exit_out 6 'no inbox item i999' "inbox close i999 exits 6" w inbox close i999
case "$(mcp list_inbox '{}' | mcp_text | tr -d '\n ')" in *'"id":"i'*) ok "MCP list_inbox returns the same items";; *) fail "MCP list_inbox" "$(mcp list_inbox '{}' | head -c 300)";; esac
eq "MCP set_inbox_item moves an item forward" "$(mcp set_inbox_item '{"id":"i2","state":"closed"}' | mcp_err)" "false"
eq "set_inbox_item with an unknown id is an error" "$(mcp set_inbox_item '{"id":"i999","state":"seen"}' | mcp_err)" "true"

# pane close closes its items
w pane close "$AGENT_PANE" >/dev/null 2>&1
has_log 'inbox item closed.*why="pane gone"' "log: closed why=pane gone"

finish
