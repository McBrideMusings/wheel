#!/usr/bin/env bash
# The permission card (wheel-h3z5.32): why the agent asks, three buttons together at the right in
# the order Deny, Always allow, Allow, and an Always allow offered with or without Claude Code's own
# suggestion. Drives the real Mac app against this script's own daemon with the fake agent; keys
# go through the app's key path (/key), the window snapshot is the window's own drawing.
# Cases: 1 reason above the command; 2 suggestions -> Always allow hands the rule back;
# 3 no suggestions -> Always allow offered, the built rule shown in its tooltip, the next matching
# command is not asked; 4 Return allows, ⌘Return always allows, Esc denies; 5 the buttons sit at the
# right in that order (WHEEL_SHOTS=<dir> keeps the PNGs; run once per WHEEL_APPEARANCE=light|dark);
# 6 a Codex request carries its own reason and allow-always option (cargo test -p wheeld --test
# agents codex_permission, run by the caller); 7 the iPhone card (tools/ios-checks.sh part).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
LOOK="${WHEEL_APPEARANCE:-light}"
mkdir -p "$SHOTS"

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
mkdir -p "$SCR/a"
A="$(w space new "$SCR/a")"
launch_app
wait_st --arg s "$A" '.active == $s' 20
P="$(st --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
w pane start "$P" agent >/dev/null
wait_st --arg p "$P" '.panes[$p].kind == "agent"' 15
S="$(st --arg p "$P" '.panes[$p].session')"

perm() { st --arg s "$S" '[.perms[] | select(.session == $s)][0]'; }
waiting() { wait_st --arg s "$S" '[.perms[] | select(.session == $s)] | length == 1' 15; }
gone() { wait_st --arg s "$S" '[.perms[] | select(.session == $s)] | length == 0' 15; }
send() { curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" --data "$1" "http://127.0.0.1:$DEBUG_PORT/agent?session=$S" >/dev/null; }
focus_composer() { app_post "/agent/focus?session=$S" >/dev/null; sleep 0.3; }
said() { st --arg p "$P" '[.panes[$p].transcript[] | tostring] | join("\n")' | grep -qE -- "$1"; }
shot() { sleep 2; app_post "/window/snapshot?path=$SHOTS/permission-card-$1-$LOOK.png" >/dev/null; echo "     shot $SHOTS/permission-card-$1-$LOOK.png"; }

# Cases 1 and 5: a Bash request with the agent's description, decision reason and blocked path.
send "permission described"
waiting
eq "1: the request carries the agent's reason" "$(perm | jq -r .reason)" "Remove the build output so the next build starts clean"
eq "1: the request carries Claude Code's decision reason" "$(perm | jq -r .decision_reason)" "Bash commands need approval in this mode"
eq "1: the request carries the blocked path" "$(perm | jq -r .blocked_path)" "/work/build"
eq "1: the command stays the title" "$(perm | jq -r .title)" "rm -rf build"
eq "2: Claude Code's suggested rule is the always rule" "$(perm | jq -r .always_rule)" "Bash"
eq "2: the options are Allow, Always allow, Deny" "$(perm | jq -r '[.options[].name] | join(",")')" "Allow,Always allow,Deny"
shot suggestion
# Case 4: Escape denies.
focus_composer
app_post "/key?keys=escape" >/dev/null
gone
check "4: Esc denied the request" poll 10 said 'permission: deny'

# Cases 2 and 4: ⌘Return always allows with Claude Code's rule.
send "permission described"
waiting
focus_composer
app_post "/key?keys=cmd+enter" >/dev/null
gone
check "2/4: ⌘Return answered Always allow, the rule went back to the agent" poll 10 said 'permission: allow always'

# Cases 3 and 4: no suggestions; the rule is Wheel's, shown before it is pressed; Return allows once.
send "permission nosuggest twice"
waiting
eq "3: Always allow is offered with no suggestions" "$(perm | jq -r '[.options[].name] | join(",")')" "Allow,Always allow,Deny"
eq "3: the rule Wheel built for the command prefix" "$(perm | jq -r .always_rule)" "Bash(rm:*)"
eq "3: the Always allow tooltip shows the rule before it is pressed" "$(perm | jq -r '.options[] | select(.kind == "allow_always") | .help')" "Always allow: adds the rule Bash(rm:*) (⌘↩)"
eq "4: the Allow tooltip names Return" "$(perm | jq -r '.options[] | select(.kind == "allow_once") | .help')" "Allow (↩)"
eq "4: the Deny tooltip names Esc" "$(perm | jq -r '.options[] | select(.kind == "reject_once") | .help')" "Deny (Esc)"
shot built-rule
focus_composer
app_post "/key?keys=cmd+enter" >/dev/null
check "3: the next matching command is not asked (it ran with the session rule)" poll 15 said 'permission again: skipped'
eq "3: nothing waits after the rule" "$(st '.perms | length')" "0"

send "permission"
waiting
focus_composer
app_post "/key?keys=enter" >/dev/null
gone
check "4: Return allowed once" poll 10 said 'permission: allow(\\n|$)'

logged() { sed 's/\x1b\[[0-9;]*m//g' "$(LOG)" | grep -q "$1"; }
check "the daemon log names the built rule" logged 'perm requested.*always_rule="Bash(rm:\*)"'
check "the daemon log gives the reason's size" logged 'perm requested.*reason_chars=54'

finish
