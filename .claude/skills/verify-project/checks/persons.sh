#!/usr/bin/env bash
# § 3c The person's panes: created_by, the permission gate on panes the person made, the CLI
# never gated, no_focus on the tools that make panes.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

STATE="$SCR/spaces.json"
PROJ="$SCR/proj"
mkdir -p "$PROJ"
start_daemon --lead-cmd "$FAKE_AGENT" --lead-cwd /tmp --state-file "$STATE"
lead_ready() { [ "$(w status | jget lead.phase)" = ready ]; }
poll 30 lead_ready || fail "the lead is ready" "$(w status | jget lead.phase)"

iserr() { mcp "$@" | mcp_err; }
text() { mcp "$@" | mcp_text; }
by() { w pane ls --json | grep -A3 "\"pane\": \"$1\"" | grep '"created_by"' | sed 's/.*: "\(.*\)".*/\1/'; }
perm_title() { w perm list | jget 0.title; }

S1="$(w space new "$PROJ")"
A="$(w pane ls | awk '{print $2}')"
eq "a pane made by space new says person" "$(by "$A")" "person"

# the lead's tools make panes that say lead
B="$(text split_pane "{\"pane\":\"$A\",\"dir\":\"right\"}")"
check "split_pane returns a pane id" test -n "$B"
eq "split_pane's pane says lead" "$(by "$B")" "lead"
C="$(w pane split "$A" down)"
eq "a CLI split says person" "$(by "$C")" "person"
R="$(text start_pane "{\"space\":\"$S1\",\"kind\":\"terminal\"}")"
D="$(printf %s "$R" | jget pane)"
eq "start_pane {space, kind} makes a pane saying lead" "$(by "$D")" "lead"
A_KEEP="$A"

# restart: same values, and the file holds created_by
restart_daemon
poll 30 lead_ready
eq "after a restart the person's pane still says person" "$(by "$A")" "person"
eq "...and the lead's pane still says lead" "$(by "$B")" "lead"
check "the file holds created_by" grep -q created_by "$STATE"

# the gate: a verb on a pane the person made raises one request with a title naming the tool and pane
gate() {  # gate <choice> <tool> <args> -> prints the reply; the request is answered in the background
  mcp_asked "$1" "$2" "$3"
}
BEFORE="$(w space ls)"
(MCP_TIMEOUT=20 mcp close_pane "{\"pane\":\"$C\"}" >"$SCR/close.out") & CP=$!; BG_PIDS+=("$CP")
poll 15 perm_id
eq "close_pane on a person's pane raises a request titled for the pane" "$(perm_title)" "close_pane on your pane $C"
eq "...from session lead" "$(w perm list | jget 0.session)" "lead"
w perm answer "$(perm_id)" deny >/dev/null 2>&1
wait "$CP" 2>/dev/null
eq "deny returns isError" "$(jget result.isError <"$SCR/close.out")" "true"
case "$(jget result.content.0.text <"$SCR/close.out")" in *"the person denied close_pane on $C"*) ok "...the person denied close_pane on $C";; *) fail "deny text" "$(cat "$SCR/close.out")";; esac
eq "the layout is byte-identical after a deny" "$(w space ls)" "$BEFORE"

R="$(gate allow close_pane "{\"pane\":\"$C\"}")"
eq "allow lets the close through" "$(printf %s "$R" | mcp_err)" "false"
PANES_NOW="$(w pane ls)"; check "...and the pane is gone (pane ls still lists the others)" bash -c "[ -n \"$PANES_NOW\" ] && ! printf %s \"$PANES_NOW\" | grep -q ' $C '"
has_log 'pane gate verb="close_pane".*asked=true' "log: pane gate close_pane asked=true"

# several verbs on a person's pane each raise a request
w pane kind "$A" terminal >/dev/null
for tool in read_terminal; do
  (mcp "$tool" "{\"pane\":\"$A\"}" >"$SCR/g.out") & GP=$!; BG_PIDS+=("$GP")
  poll 15 perm_id
  case "$(perm_title)" in "$tool on your pane $A"*) ok "$tool on a person's pane raises '$tool on your pane $A'";; *) fail "$tool gate title" "$(perm_title)";; esac
  w perm answer "$(perm_id)" deny >/dev/null 2>&1
  wait "$GP" 2>/dev/null
done
(mcp type_into_pane "{\"pane\":\"$A\",\"text\":\"echo typed-\$((6*7))\",\"enter\":true}" >"$SCR/t.out") & TP=$!; BG_PIDS+=("$TP")
poll 15 perm_id
case "$(perm_title)" in "type_into_pane on your pane $A: \"echo typed-"*) ok "type_into_pane's title quotes the text";; *) fail "type_into_pane title" "$(perm_title)";; esac
w perm answer "$(perm_id)" deny >/dev/null 2>&1
wait "$TP" 2>/dev/null

# the lead's own panes ask nothing
R="$(mcp close_pane "{\"pane\":\"$D\"}")"
eq "close_pane on the lead's own pane runs with no request" "$(printf %s "$R" | mcp_err)" "false"
eq "perm list stays empty" "$(w perm list)" "[]"

# errors before anyone is asked
eq "a pane that does not exist is an error with nothing asked" "$(iserr close_pane '{"pane":"p999"}')" "true"
eq "...and perm list is still empty" "$(w perm list)" "[]"

# the CLI is the person and is never gated
expect_exit 0 "wheel pane kind on a person's pane raises nothing" w pane kind "$A" empty
eq "...perm list is empty" "$(w perm list)" "[]"
expect_exit 0 "wheel pane close on a person's pane raises nothing" w pane close "$A"
eq "...perm list is empty" "$(w perm list)" "[]"

# no_focus: split_pane defaults to leaving the person's focus alone
S2="$(w space new "$PROJ")"
P2="$(w space ls | jget spaces.1.focus)"
w pane focus "$P2" >/dev/null
ACTIVE="$(w space ls | jget active)"
FOCUS_A="$(w space ls | jget spaces.1.focus)"
text split_pane "{\"pane\":\"$P2\",\"dir\":\"right\"}" >/dev/null
eq "split_pane leaves the active space alone" "$(w space ls | jget active)" "$ACTIVE"
eq "...and the focus where it was" "$(w space ls | jget spaces.1.focus)" "$FOCUS_A"
has_log 'pane split.*by=Lead.*focus=false' "log: pane split by=Lead focus=false"
has_log 'space created.*by=[Pp]erson' "log: space created by=Person"
R="$(mcp split_pane "{\"pane\":\"$P2\",\"dir\":\"down\",\"no_focus\":false}")"
NEWP="$(printf %s "$R" | mcp_text)"
eq "no_focus: false focuses the new pane" "$(w space ls | jget spaces.1.focus)" "$NEWP"

# a hand-written state file with no created_by loads as person, and the lead then asks before closing
stop_daemon
rm -f "$STATE"
start_daemon --no-lead --state-file "$STATE"
w space new "$PROJ" >/dev/null
stop_daemon
sed -i '' '/"created_by"/d' "$STATE"
start_daemon --lead-cmd "$FAKE_AGENT" --lead-cwd /tmp --state-file "$STATE"
poll 30 lead_ready
OLD="$(w pane ls | awk '{print $2}')"
eq "a state file with no created_by loads person" "$(by "$OLD")" "person"
(mcp close_pane "{\"pane\":\"$OLD\"}" >"$SCR/o.out") & OP=$!; BG_PIDS+=("$OP")
poll 15 perm_id
eq "...and the lead asks before closing it" "$(perm_title)" "close_pane on your pane $OLD"
w perm answer "$(perm_id)" deny >/dev/null 2>&1
wait "$OP" 2>/dev/null

finish
