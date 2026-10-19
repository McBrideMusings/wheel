#!/usr/bin/env bash
# § 2b Spaces and the split tree: ids, splits, focus after a close, restart, unknown kinds kept.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

STATE="$SCR/spaces.json"
PROJ="$SCR/proj"
mkdir -p "$PROJ"
start_daemon --no-lead --state-file "$STATE"

ls_field() { w space ls | jget "$1"; }

eq "space new prints s1" "$(w space new "$PROJ")" "s1"
eq "one space" "$(ls_field spaces)" "1"
eq "the space is named for its directory" "$(ls_field spaces.0.name)" "proj"
eq "active is s1" "$(ls_field active)" "s1"
A="$(ls_field spaces.0.root.id)"
eq "one empty pane" "$(ls_field spaces.0.root.kind)" "empty"
eq "that pane is the space's focus" "$(ls_field spaces.0.focus)" "$A"

B="$(w pane split "$A" right)"
check "pane split right prints the new pane id" test -n "$B"
eq "the new pane takes the focus" "$(ls_field spaces.0.focus)" "$B"
SPLIT="$(ls_field spaces.0.root.id)"
expect_exit 0 "pane kind terminal exits 0" w pane kind "$B" terminal
expect_exit 0 "pane ratio 0.3 exits 0" w pane ratio "$SPLIT" 0.3
expect_exit 0 "pane focus on the first pane exits 0" w pane focus "$A"
eq "focus moved to the first pane" "$(ls_field spaces.0.focus)" "$A"
expect_exit 2 "pane split diagonal exits 2" w pane split "$A" diagonal
expect_exit 6 "pane close p999 exits 6" w pane close p999

# Restart on the same state file: same ids, ratio, focus; the terminal pane gets a new term, alive, in the space's cwd.
TERM_BEFORE="$(ls_field spaces.0.root.b.term)"
restart_daemon
eq "restart keeps the split id" "$(ls_field spaces.0.root.id)" "$SPLIT"
eq "restart keeps the first pane id" "$(ls_field spaces.0.root.a.id)" "$A"
eq "restart keeps the second pane id" "$(ls_field spaces.0.root.b.id)" "$B"
eq "restart keeps ratio 0.3" "$(ls_field spaces.0.root.ratio | awk '{printf "%.2f", $1}')" "0.30"
eq "restart keeps the focus" "$(ls_field spaces.0.focus)" "$A"
TERM_AFTER="$(ls_field spaces.0.root.b.term)"
check "the terminal pane has a term id" test -n "$TERM_AFTER"
check "the term id is new" test "$TERM_AFTER" != "$TERM_BEFORE"
eq "status lists that terminal alive" "$(w status | jget terms.0.alive)" "true"
eq "...in the space's cwd" "$(w status | jget terms.0.cwd)" "$PROJ"
w term send "$TERM_AFTER" 'echo pane-$((6*7))' --enter
expect_exit 0 "the restored terminal answers (pane-42)" w term read "$TERM_AFTER" --until pane-42 --timeout 20
has_log "spaces restored.*respawned=1" "log: spaces restored respawned=1"
has_log "space created" "log: space created"
has_log "pane split" "log: pane split"
has_log "pane kind" "log: pane kind"
has_log "split ratio" "log: split ratio"

# Closing the focused pane focuses the pane beside it.
w space new "$PROJ" >/dev/null
P1="$(w space ls | jget spaces.1.root.id)"
P2="$(w pane split "$P1" right)"     # P1 | P2
P3="$(w pane split "$P2" down)"      # P1 | (P2 / P3), focus P3
w pane close "$P3" >/dev/null
eq "A | (B / C), C closed: focus is B" "$(w space ls | jget spaces.1.focus)" "$P2"
has_log "pane closed.*focus=$P2" "log: pane closed focus=$P2"

w space new "$PROJ" >/dev/null
Q1="$(w space ls | jget spaces.2.root.id)"
Q3="$(w pane split "$Q1" right)"     # Q1 | Q3
Q2="$(w pane split "$Q1" down)"      # (Q1 / Q2) | Q3
w pane focus "$Q3" >/dev/null
w pane close "$Q3" >/dev/null
eq "(A / B) | C, C closed: focus is B" "$(w space ls | jget spaces.2.focus)" "$Q2"

# Closing both panes of a space closes the space.
w pane close "$Q1" >/dev/null
w pane close "$Q2" >/dev/null
has_log "pane closed; it was the last, so space closed" "log: last pane closes its space"
eq "two spaces remain" "$(w space ls | jget spaces)" "2"
w pane close "$P1" >/dev/null; w pane close "$P2" >/dev/null
w pane close "$A" >/dev/null; w pane close "$B" >/dev/null
eq "closing every pane leaves no spaces" "$(w space ls | jget spaces)" "0"
has_log "pty killed" "log: pty killed when a terminal pane closes"
check "active is null" bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' space ls | grep -q '\"active\": null'"

# A pane kind this build does not know is kept whole.
stop_daemon
rm -f "$STATE"
start_daemon --no-lead --state-file "$STATE"
w space new "$PROJ" >/dev/null
stop_daemon
sed -i '' 's/"kind": "empty"/"kind": "hologram", "depth": 3/' "$STATE"
start_daemon --no-lead --state-file "$STATE"
HP="$(w space ls | jget spaces.0.root.id)"
eq "the unknown kind is shown unchanged (kind)" "$(w space ls | jget spaces.0.root.kind)" "hologram"
eq "the unknown kind is shown unchanged (depth)" "$(w space ls | jget spaces.0.root.depth)" "3"
w space new "$PROJ" >/dev/null
check "still in the file after another change" grep -q hologram "$STATE"
has_log "unknown pane kind; kept as is.*kind=hologram" "log: unknown pane kind; kept as is"
expect_exit 0 "pane kind p -> terminal exits 0" w pane kind "$HP" terminal
expect_exit 0 "pane close on it exits 0" w pane close "$HP"

# An extra top-level key survives.
stop_daemon
rm -f "$STATE"
start_daemon --no-lead --state-file "$STATE"
w space new "$PROJ" >/dev/null
stop_daemon
sed -i '' 's/"next_id"/"future_key": {"a": [1, 2]}, "next_id"/' "$STATE"
start_daemon --no-lead --state-file "$STATE"
has_log "unknown saved key; kept as is.*key=future_key" "log: unknown saved key; kept as is"
w space new "$PROJ" >/dev/null
check "the file still holds future_key after a change" bash -c "plutil -extract future_key.a.1 raw -o - '$STATE' | grep -qx 2"

finish
