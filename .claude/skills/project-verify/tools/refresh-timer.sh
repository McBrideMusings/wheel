#!/usr/bin/env bash
# A refresh pane's timer sleeps until its next run instead of waking every second, and an edited
# `every` still moves the next run at once (wheel-qdrj.5).
source "$(dirname "$0")/lib.sh"

PROJ="$SCR/proj"
mkdir -p "$PROJ/site"
printf 'hi\n' >"$PROJ/site/index.html"
entry() { printf '[artifacts.site]\npath = "site"\ncommand = "date +%%s >> \\"$WHEEL_DATA_DIR/ticks\\""\nevery = %s\n' "$1" >"$PROJ/wheel.toml"; }
entry 2

start_daemon
w space new "$PROJ" >/dev/null
P="$(w space ls --json | plutil -extract 0.panes.0 raw -o - -- -)"
w pane kind "$P" web --artifact site >/dev/null
TICKS="$INST/panes/$P/ticks"
count() { [ -f "$TICKS" ] && wc -l <"$TICKS" | tr -d ' ' || echo 0; }

sleep 7
n="$(count)"
# The opening run, then one every 2 s: 4 in 7 s, give or take one for start-up.
[ "$n" -ge 3 ] && [ "$n" -le 5 ] && ok "every = 2 ran $n times in 7 s" || fail "every = 2 ran $n times in 7 s, want 3-5"

entry 1
before="$(count)"
sleep 5
after="$(count)"
gained=$((after - before))
# The next run rereads the entry; from then on one a second.
[ "$gained" -ge 4 ] && ok "after every = 1, $gained runs in 5 s" || fail "after every = 1, $gained runs in 5 s, want >= 4"

echo "$PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
