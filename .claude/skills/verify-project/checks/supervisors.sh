#!/usr/bin/env bash
# § 3e Supervisors (ADR-0007): start/show/queue, state verbs, landing a worker's branch on main,
# the loop's decisions, persistence. The fake agent plays supervisor and worker; its behaviour is
# chosen by the bead titles below.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

STATE="$SCR/spaces.json"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t

# mkrepo <dir> [check command]: a git repo with one commit and beads (prefix t); a project check when given.
mkrepo() {
  mkdir -p "$1"
  git -C "$1" init -q -b main
  git -C "$1" commit -q --allow-empty -m init
  if [ -n "${2:-}" ]; then
    printf '[project]\ncheck = "%s"\n' "$2" >"$1/wheel.toml"
    git -C "$1" add wheel.toml
    git -C "$1" commit -q -m "project check"
  fi
  ( cd "$1" && bd init --prefix t --quiet >/dev/null 2>&1 )
}
# bdr <repo> <bd args...>: bd in that repo.
bdr() { local r="$1"; shift; ( cd "$r" && bd "$@" ); }
# bead <repo> <title> [parent] [dep]: creates a bead, prints its id.
bead() {
  local args=("$2" --json)
  [ -n "${3:-}" ] && args+=(--parent "$3")
  [ -n "${4:-}" ] && args+=(--deps "$4")
  bdr "$1" create "${args[@]}" | jget id
}
epic() { bdr "$1" create "$2" --type epic --json | jget id; }

# kinds <supervisor id>: the event kinds in order, space separated.
kinds() { w supervisor events "$1" | tr -d '\n ' | grep -o '"kind":"[a-z_]*"' | cut -d'"' -f4 | tr '\n' ' ' | sed 's/ $//'; }
# kinds_for <supervisor id> <bead>: the event kinds that name one bead.
kinds_for() { w supervisor events "$1" | tr -d '\n ' | sed 's/},{/}\n{/g' | grep "\"bead\":\"$2\"" | grep -o '"kind":"[a-z_]*"' | cut -d'"' -f4 | tr '\n' ' ' | sed 's/ $//'; }
lead_ready() { [ "$(w status | jget lead.phase)" = ready ]; }
sup_field() { w supervisor show "$1" | jget "$2"; }

start_daemon --lead-cmd "$FAKE_AGENT" --lead-cwd /tmp --state-file "$STATE"
poll 30 lead_ready || fail "the lead is ready" "$(w status | jget lead.phase)"

# --- scope, queue, state verbs ---------------------------------------------------------------
R1="$SCR/repo1"; mkrepo "$R1"
E="$(epic "$R1" "epic E")"
A="$(bead "$R1" "task a" "$E")"
B="$(bead "$R1" "task b" "$E" "$A")"
C="$(bead "$R1" "task c" "$E")"
OUTSIDE="$(bead "$R1" "outside the epic")"

w space new "$R1" >/dev/null
SUP="$(w supervisor start --goal "$E" --workers 3 --cap 60)"
SID="$(printf %s "$SUP" | jget id)"
case "$SID" in "$E"-u*) ok "start prints a supervisor id: the goal plus a short id";; *) fail "supervisor id" "$SID";; esac
eq "workers 3" "$(printf %s "$SUP" | jget workers)" "3"
eq "cap 60" "$(printf %s "$SUP" | jget cap)" "60"
eq "state running" "$(printf %s "$SUP" | jget state)" "running"
for f in session pane space; do check "start carries a $f" test -n "$(printf %s "$SUP" | jget $f)"; done
expect_exit_out 6 'needs a goal' "start with no --goal or --focus exits 6" w supervisor start

queued() { [ -n "$(sup_field "$SID" queue.1.bead)" ]; }
poll 30 queued
Q0="$(sup_field "$SID" queue.0.bead)"; Q1="$(sup_field "$SID" queue.1.bead)"
eq "the queue holds exactly two beads" "$(sup_field "$SID" queue)" "2"
case "$Q0 $Q1" in *"$A"*"$C"*|*"$C"*"$A"*) ok "...A and C, the epic's ready beads";; *) fail "queue holds A and C" "$Q0 $Q1";; esac
case " $Q0 $Q1 " in *" $B "*) fail "B (blocked) is absent from the queue";; *) ok "B (blocked) is absent from the queue";; esac
case " $Q0 $Q1 " in *" $OUTSIDE "*) fail "a bead outside the epic is absent";; *) ok "a bead outside the epic is absent";; esac
has_log "supervisor scope read.*ready=2" "log: supervisor scope read ready=2"
has_log "supervisor agent answered" "log: supervisor agent answered"
has_log "supervisor queue stored.*queued=2" "log: supervisor queue stored queued=2"
has_log "agent config linked.*supervisor-home" "log: agent config linked supervisor-home"

# the verbs print the supervisor with the change
eq "pause" "$(w supervisor pause "$SID" | jget state)" "paused"
expect_exit_out 6 'is paused' "pause twice exits 6" w supervisor pause "$SID"
case "$(w supervisor resume "$SID" | jget state)" in running|held) ok "resume leaves it running or held";; *) fail "resume";; esac
eq "workers 5" "$(w supervisor workers "$SID" 5 | jget workers)" "5"
eq "cap 70" "$(w supervisor cap "$SID" 70 | jget cap)" "70"
eq "cap with no number removes the cap" "$(w supervisor cap "$SID" | jget cap)" ""
expect_exit_out 6 'no supervisor' "show nope-u9 exits 6" w supervisor show nope-u9
eq "list prints the supervisor" "$(w supervisor list | jget 0.id)" "$SID"
eq "status carries the supervisor pane" "$(w status | tr -d '\n ' | grep -o '"kind":"supervisor"' | head -1)" '"kind":"supervisor"'
has_log "supervisor changed" "log: supervisor changed"

# the lead's MCP tools print the same JSON
eq "MCP list_supervisors" "$(mcp list_supervisors '{}' | mcp_err)" "false"
eq "MCP show_supervisor" "$(mcp show_supervisor "{\"id\":\"$SID\"}" | mcp_text | jget id)" "$SID"

# persistence across a restart
restart_daemon
poll 30 lead_ready
eq "a restart lists the same supervisor" "$(w supervisor list | jget 0.id)" "$SID"
eq "...with the same workers" "$(sup_field "$SID" workers)" "5"
check "the saved layout holds the whole supervisor" bash -c "tr -d '\n ' <'$STATE' | grep -q '\"kind\":\"supervisor\"'"

eq "stop" "$(w supervisor stop "$SID" | jget state)" "stopped"
eq "...a stopped supervisor has no agent" "$(sup_field "$SID" agent)" ""
for verb in pause resume; do
  expect_exit_out 6 'stopped' "$verb after stop exits 6" w supervisor "$verb" "$SID"
done
expect_exit_out 6 'stopped' "workers after stop exits 6" w supervisor workers "$SID" 2
expect_exit_out 6 'stopped' "cap after stop exits 6" w supervisor cap "$SID" 50

# --- landing ---------------------------------------------------------------------------------------
stop_daemon
rm -f "$STATE"
start_daemon --lead-cmd "$FAKE_AGENT" --lead-cwd /tmp --state-file "$STATE"
poll 30 lead_ready
R2="$SCR/repo2"; mkrepo "$R2" "test -f a.txt"
E2="$(epic "$R2" "landing")"
L="$(bead "$R2" "close write a.txt" "$E2")"
w space new "$R2" >/dev/null
S2="$(w supervisor start --goal "$E2" --workers 1 | jget id)"
w supervisor cap "$S2" >/dev/null
landed() { kinds_for "$S2" "$L" | grep -q landed; }
poll 90 landed
eq "a landed bead's events run claimed started landed" "$(kinds_for "$S2" "$L")" "claimed started landed"
DETAIL="$(w supervisor events "$S2" | tr -d '\n' | sed 's/},  {/}\n{/g' | grep '"kind": "landed"' | sed 's/.*"detail": "\(.*\)".*/\1/')"
MAIN_SHA="$(git -C "$R2" rev-parse main)"
case "$DETAIL" in *"branch $L landed on main at $MAIN_SHA"*) ok "the landed detail names the sha git rev-parse main reports";; *) fail "landed detail" "$DETAIL (main $MAIN_SHA)";; esac
case "$DETAIL" in *passed*) ok "...and the project check passed";; *) fail "check passed" "$DETAIL";; esac
check "a.txt exists in the repo" test -f "$R2/a.txt"
wt_gone() { ! git -C "$R2" worktree list | grep -q "worktrees/s1/$L"; }
poll 15 wt_gone
if ! wt_gone; then fail "the bead's worktree is gone"; else ok "the bead's worktree is gone"; fi
eq "bd reports the bead closed" "$(bdr "$R2" show "$L" --json | jget 0.status)" "closed"
has_log "loop event.*kind=Landed" "log: loop event kind=Landed"
w supervisor stop "$S2" >/dev/null

# --- a red check is sent back, then escalated, and main does not move ---------------------------
R3="$SCR/repo3"; mkrepo "$R3" "echo boom; exit 1"
E3="$(epic "$R3" "red")"
RB="$(bead "$R3" "close write a.txt" "$E3")"
w space new "$R3" >/dev/null
S3="$(w supervisor start --goal "$E3" --workers 1 | jget id)"
w supervisor cap "$S3" >/dev/null
MAIN0="$(git -C "$R3" rev-parse main)"
escalated() { kinds_for "$S3" "$RB" | grep -q escalated; }
poll 120 escalated
case "$(kinds_for "$S3" "$RB")" in "claimed started sent_back"*escalated*) ok "a red check gives sent_back then escalated";; *) fail "red check events" "$(kinds_for "$S3" "$RB")";; esac
check "sent_back carries exited 1 and boom" bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' supervisor events '$S3' | tr -d '\n' | grep -q 'exited 1.*boom'"
eq "main does not move" "$(git -C "$R3" rev-parse main)" "$MAIN0"
w supervisor stop "$S3" >/dev/null

# --- the loop: landed, blocked, continued until escalated ---------------------------------------
R4="$SCR/repo4"; mkrepo "$R4"
E4="$(epic "$R4" "loop")"
LB1="$(bead "$R4" "close it" "$E4")"
LB2="$(bead "$R4" "block me" "$E4")"
LB3="$(bead "$R4" "stubborn" "$E4")"
w space new "$R4" >/dev/null
S4="$(w supervisor start --goal "$E4" --workers 1 | jget id)"
w supervisor cap "$S4" >/dev/null
loopdone() { kinds_for "$S4" "$LB3" | grep -q escalated; }
poll 150 loopdone; sleep 2
eq "close it: claimed started landed" "$(kinds_for "$S4" "$LB1")" "claimed started landed"
case "$(kinds_for "$S4" "$LB2")" in "claimed started blocked"*) ok "block me: claimed started blocked...";; *) fail "block me events" "$(kinds_for "$S4" "$LB2")";; esac
case "$(kinds_for "$S4" "$LB3")" in "claimed started continued continued continued escalated"*) ok "stubborn: claimed started continued x3 escalated (then the supervisor is woken)";; *) fail "stubborn events" "$(kinds_for "$S4" "$LB3")";; esac
check "the started detail names the worktree under the support dir" bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' supervisor events '$S4' | grep -q 'worktrees/'"
eq "bd: close it is closed" "$(bdr "$R4" show "$LB1" --json | jget 0.status)" "closed"
eq "bd: block me is blocked" "$(bdr "$R4" show "$LB2" --json | jget 0.status)" "blocked"
eq "bd: stubborn is in_progress" "$(bdr "$R4" show "$LB3" --json | jget 0.status)" "in_progress"
check "git branch lists a branch per bead" bash -c "git -C '$R4' branch | grep -q '$LB1' && git -C '$R4' branch | grep -q '$LB2' && git -C '$R4' branch | grep -q '$LB3'"
check "supervisor inbox lists the blocked and escalated items" bash -c "test \$('$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' supervisor inbox '$S4' | grep -c '\"bead\"') -ge 2"
eq "supervisor show has inbox: 2" "$(sup_field "$S4" inbox)" "2"
w supervisor stop "$S4" >/dev/null

finish
