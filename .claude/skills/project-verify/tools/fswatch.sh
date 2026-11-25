#!/usr/bin/env bash
# One file-event stream for the daemon: six web panes and an agent watch through one FSEvents
# thread, and a change to the artifact still reloads its panes (wheel-qdrj.2).
source "$(dirname "$0")/lib.sh"

PROJ="$SCR/proj"
mkdir -p "$PROJ/site"
printf 'hi\n' >"$PROJ/site/index.html"
printf '[artifacts.site]\npath = "site"\n' >"$PROJ/wheel.toml"

use_fake_agent
start_daemon

w space new "$PROJ" >/dev/null
sleep 1
nthreads() { ps -M -p "$DPID" | tail -n +2 | wc -l | tr -d ' '; }
before="$(nthreads)"
P="$(w space ls --json | plutil -extract 0.panes.0 raw -o - -- -)"
w pane kind "$P" web --artifact site >/dev/null
panes=("$P")
for i in 1 2 3 4 5; do
  N="$(w pane split "${panes[-1]}" right)"
  w pane kind "$N" web --artifact site >/dev/null
  panes+=("$N")
done
A="$(w pane split "$P" down)"
w pane start "$A" agent --message hi >/dev/null
sleep 2

after="$(nthreads)"
old="$(sample "$DPID" 1 2>/dev/null | grep -cE 'Thread_[0-9]+: notify-rs fsevents loop')"
# Each pane used to bring 3-5 watcher threads of its own; now its folders join the shared queue.
[ $((after - before)) -le 4 ] && ok "six web panes and an agent added $((after - before)) threads ($before -> $after)" \
  || fail "six web panes and an agent added $((after - before)) threads ($before -> $after), want at most 4"
[ "$old" = 0 ] && ok "no per-watch notify-rs fsevents threads" || fail "notify-rs fsevents threads: $old, want 0"

before="$(grep -c 'pane reload' "$(LOG)")"
printf 'changed\n' >"$PROJ/site/index.html"
for _ in $(seq 1 50); do
  [ "$(grep -c 'pane reload' "$(LOG)")" -gt "$before" ] && break
  sleep 0.1
done
after="$(grep -c 'pane reload' "$(LOG)")"
[ "$after" -gt "$before" ] && ok "an edit to the artifact reloads its panes ($((after - before)) reloads)" \
  || fail "no pane reload after editing the artifact"

echo "$PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
