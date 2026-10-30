#!/usr/bin/env bash
# § `wheel artifact promote` makes an ad hoc artifact permanent (wheel-3svn): the CLI against a running
# wheeld asks a person, who answers on the Mac app's card (the debug server's /perm, the card's own
# path). A denial changes nothing; an allow rewrites the entry in the target scope with no
# ends_with_pane, no expires_at and pane = true, leaves the folder where it was, and removes the entry
# from the scope it left. Drives the real Mac app against this script's own daemon and the fake agent.
# WHEEL_SHOTS=<dir> keeps the window snapshot of the waiting card.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"
LOOK="${WHEEL_APPEARANCE:-light}"

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
mkdir -p "$SCR/proj1"
S1="$(w space new "$SCR/proj1")"
P="$(w space ls | jq -r --arg s "$S1" '[.spaces[] | select(.id == $s)][0] | [.. | objects | select(.id? and .kind?) | .id] | first')"
w pane start "$P" agent --message "hello" >/dev/null
w agent wait "$P" --timeout 10 >/dev/null

prefs_reset
launch_app
wait_st '.windows | length >= 1' 20
w space focus "$S1" >/dev/null 2>&1 || true

# entry <name> <scope>: that artifact's listing row, as compact JSON ("null" when it has none).
entry() { w artifact ls --space "$S1" --json | jq -c --arg n "$1" --arg s "$2" '(.artifacts // .) | [.[] | select(.name == $n and .scope == $s)] | first'; }
field() { entry "$1" "$2" | jq -r ".$3"; }

# promote <name> <choice> <promote args...>: run the CLI in the background, wait for the card in the
# app, answer it with <choice> through the app, and leave the CLI's exit code in $RC and its output in $OUT.
promote() {
  local name="$1" choice="$2"; shift 2
  w artifact promote "$name" --space "$S1" --from "$P" "$@" >"$SCR/promote.out" 2>&1 &
  local pid=$!
  BG_PIDS+=("$pid")
  check "the app shows the request for $name" wait_st '.waiting_count == 1' 10
  PERM_TITLE="$(st '.perms[0].title')"
  echo "     asking session: $(st '.perms[0].session'); the pane's request: $(st --arg p "$P" '.panes[$p].request')"
  check "the request is the card on the asking agent pane" wait_st ".panes[\"$P\"].request == .perms[0].id" 10
  sleep "${WAIT:-1}"
  echo "     card: $PERM_TITLE"
  PERM_OPTS="$(st -c '[.perms[0].options[].id]')"
  [ -z "${SHOT:-}" ] || { app_post "/window/snapshot?path=$SHOTS/$SHOT-$LOOK.png" >/dev/null && echo "     shot $SHOTS/$SHOT-$LOOK.png"; SHOT=""; }
  app_post "/perm?id=$(st '.perms[0].id')&choice=$choice" >/dev/null
  wait "$pid"; RC=$?
  OUT="$(cat "$SCR/promote.out")"
  echo "     cli exit $RC: $OUT" | head -3
}

# 1. A project entry that ends with its agent pane, denied after 12 s (longer than the CLI's ordinary
# ten-second wait for a reply): nothing changes.
WHEEL_PANE="$P" w artifact new session-page --space "$S1" --ends-with-session >/dev/null
BEFORE="$(entry session-page project)"
echo "     before: $BEFORE"
eq "the ad hoc entry ends with its pane" "$(field session-page project ends_with_pane)" "$P"
WAIT=12 SHOT=promote-card promote session-page deny --to machine
eq "denied: the CLI exits non-zero" "$([ "$RC" != 0 ] && echo yes)" yes
expect_out "denied; nothing changed" "denied: the CLI says nothing changed" echo "$OUT"
eq "the card offered allow and deny" "$PERM_OPTS" '["allow","deny"]'
eq "denied: the project entry is exactly as it was" "$(entry session-page project)" "$BEFORE"
eq "denied: no machine entry appeared" "$(entry session-page machine)" null
eq "denied: the app's waiting count is back to 0" "$(st '.waiting_count')" 0

# 2. The same entry, allowed to project: it stays in the project scope, loses ends_with_pane, gains pane.
DIR="$(field session-page project dir)"
promote session-page allow --to project
eq "allowed to project: the CLI exits 0" "$RC" 0
eq "allowed to project: the CLI prints the folder" "$OUT" "$(printf '%s\n[promoted project artifact session-page in %s; its folder is left as it was]' "$DIR" "$(field session-page project entry)")"
eq "to project: ends_with_pane is cleared" "$(field session-page project ends_with_pane)" null
eq "to project: pane is true" "$(field session-page project pane)" true
eq "to project: still listed in the project scope" "$(field session-page project name)" session-page
eq "to project: the folder is the same" "$(field session-page project dir)" "$DIR"
check "to project: the folder's index.html is intact" test -s "$DIR/index.html"

# 3. A project entry that expires, allowed to machine: it moves, loses expires_at, keeps its folder.
w artifact new timed --space "$S1" --expires 2h >/dev/null
DIR="$(field timed project dir)"
check "the timed entry expires" test "$(field timed project expires_at)" != null
promote timed allow --to machine
eq "allowed to machine: the CLI exits 0" "$RC" 0
eq "to machine: listed in the machine scope" "$(field timed machine name)" timed
eq "to machine: expires_at is cleared" "$(field timed machine expires_at)" null
eq "to machine: pane is true" "$(field timed machine pane)" true
eq "to machine: the project scope no longer lists it" "$(entry timed project)" null
eq "to machine: the folder is the same" "$(field timed machine dir)" "$DIR"
check "to machine: the folder's index.html is intact" test -s "$DIR/index.html"

# 4. A machine entry that expires, allowed to project.
w artifact new mtimed --scope machine --expires 2h >/dev/null
DIR="$(field mtimed machine dir)"
promote mtimed allow --to project
eq "machine to project: the CLI exits 0" "$RC" 0
eq "machine to project: listed in the project scope" "$(field mtimed project name)" mtimed
eq "machine to project: expires_at is cleared" "$(field mtimed project expires_at)" null
eq "machine to project: pane is true" "$(field mtimed project pane)" true
eq "machine to project: the machine scope no longer lists it" "$(entry mtimed machine)" null
eq "machine to project: the folder is the same" "$(field mtimed project dir)" "$DIR"
check "machine to project: the folder's index.html is intact" test -s "$DIR/index.html"

# 5. Already permanent: refused before anyone is asked.
expect_exit_out 6 "already a permanent" "an already permanent entry is refused with no card" w artifact promote timed --to machine --space "$S1" --from "$P"
eq "the refusal raised no card" "$(st '.waiting_count')" 0

has_log "artifact promoted" "the daemon logged the promotions"
has_log "artifact promotion denied" "the daemon logged the denial"
finish
