#!/usr/bin/env bash
# § 2c-runner The runner built-in (wheel-9xu6): its page lists the [runner] table and starts one
# through wheel.run, exactly as the page's own script sends it, in the web pane's space even when
# another space is active.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

start_daemon --state-file "$SCR/spaces.json"
mkdir -p "$SCR/proj" "$SCR/other"
printf '[runner]\nhello = "echo runner-says-$((6*7))"\n' >"$SCR/proj/wheel.toml"
printf '[runner]\nmachine-one = "true"\n' >>"$INST/wheel.toml"
w space new "$SCR/proj" >/dev/null
S="$(w space ls | jget spaces.0.id)"
P="$(w space ls | jget spaces.0.root.id)"
B="$(BASE)/artifacts/$P"
DIR="$INST/artifacts/runner"
http() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
# A run as the page sends it: POST .wheel/run with the page's cookie.
page_run() { curl -s -b "$SCR/jar" -H 'content-type: application/json' -d "$1" "$B/.wheel/run"; }

expect_out '^runner  shipped 1  installed 1$' "the runner built-in installs" w artifact builtin install runner
expect_exit 0 "a web pane opens it" w pane kind "$P" web --artifact runner --scope machine
eq "and serves its page: 200" "$(http -L -c "$SCR/jar" "$B/index.html?token=$(TOKEN)")" "200"
check "the page talks to the page channel and lists with wheel run --json" sh -c "curl -s -b '$SCR/jar' '$B/' | grep -q '.wheel/client.js' && curl -s -b '$SCR/jar' '$B/' | grep -q 'wheel run --json'"

# Another space becomes the active one, so a run that ignored the pane's space would land there.
w space new "$SCR/other" >/dev/null
O="$(w space ls | jget spaces.1.id)"
expect_out '^machine-one' "wheel run lists the machine's runner" w run --space "$S"

LIST="$(page_run '{"command":"wheel run --json | wheel artifact data \"$WHEEL_PANE\" -","cwd":"/"}')"
eq "the page's list command exits 0" "$(printf %s "$LIST" | jget exit)" "0"
EVENTS="$(curl -s -b "$SCR/jar" --max-time 2 "$B/.wheel/events")"
check "the pushed list reaches the page with the project's runner" sh -c "printf %s '$EVENTS' | grep -q '\"name\":\"hello\"'"
check "and the machine's" sh -c "printf %s '$EVENTS' | grep -q '\"name\":\"machine-one\"'"

START="$(page_run '{"command":"wheel run \"$RUNNER\"","cwd":"/","env":{"RUNNER":"hello"}}')"
check "the page's start command started" sh -c "printf %s '$START' | grep -q '\"started\":true'"
poll 10 sh -c "\"$WHEEL\" --url ws://127.0.0.1:$PORT/ws --token-file $INST/token space ls | plutil -extract spaces.0.root.b.id raw -o - - >/dev/null 2>&1"
T="$(w space ls | jget spaces.0.root.b.id)"
eq "the new terminal pane is split right of the web pane, in its space $S" "$(w space ls | jget spaces.0.root.b.kind)" "terminal"
check "and the active space $O gained no pane" test -z "$(w space ls | jget spaces.1.root.b.id)"
expect_exit 0 "the terminal ran the runner's command" w term read "$T" --until "runner-says-42" --timeout 10
# A page that lists as it loads must not loop: neither a run's log nor the pushed list reloads it.
# (FSEvents reports the data folder's creation along with its first change, which reloads once.)
for i in 1 2 3; do page_run '{"command":"wheel run --json | wheel artifact data \"$WHEEL_PANE\" -","cwd":"/"}' >/dev/null; sleep 0.5; done
sleep 1
RELOADS="$(grep -c "pane reload pane=$P role=Data" "$(LOG)")"
check "five page runs reloaded the pane at most once (got $RELOADS)" test "$RELOADS" -le 1
has_log "page run started pane=$P space=$S" "the page run log names the pane's space"
has_log "runner started space=$S .*name=hello" "the daemon started the runner in that space"

expect_out '^runner  shipped 1  not installed$' "runner removes" w artifact builtin remove runner
finish
