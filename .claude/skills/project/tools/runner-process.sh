#!/usr/bin/env bash
# § Runner process (wheel-cmbu.8): `wheel-runner serve` is one process behind a shared machine
# entry. Two spaces on two projects each list their own [runner] commands; a run started from one
# keeps its status, exit code and log file for a later call (a reopened page), a command with a
# confirm question starts nothing until confirmed, a long run stops and says stopped, and one
# `wheel-runner serve` process serves both spaces. Needs
# `cargo build -p wheel-runner -p wheeld -p wheel-cli --examples --bins`; no app.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

RUNNER="$BIN/wheel-runner"
[ -x "$RUNNER" ] || { echo "missing $RUNNER: run cargo build -p wheel-runner" >&2; exit 2; }

export WHEEL_RUNNER_DIR="$SCR/runs"
export WHEEL_MACHINE_FILE="$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"

mkdir -p "$SCR/one" "$SCR/two"
printf '[runner]\nhello = "echo hello-from-one; exit 4"\nrestart = { command = "echo restarted-one", confirm = "Ends every agent." }\nlong = "exec sleep 30"\n' >"$SCR/one/wheel.toml"
printf '[runner]\nonly-two = "echo two"\n' >"$SCR/two/wheel.toml"
w space new "$SCR/one" >/dev/null
w space new "$SCR/two" >/dev/null
SP1="$(w space ls | jget spaces.0.id)"
SP2="$(w space ls | jget spaces.1.id)"

printf '[artifacts.runner-native]\nmcp = "exec %s serve"\nshared = true\ntitle = "Runner"\n' "$RUNNER" >"$INST/wheel.toml"

call() { local sp="$1"; shift; w artifact call runner-native "$@" --scope machine --space "$sp" --json 2>&1; }
has() { printf %s "$1" | grep -q -- "$2"; }
hasnt() { ! has "$1" "$2"; }
servers() { pgrep -P "$DPID" -f "$RUNNER serve" | wc -l | tr -d ' '; }

L1="$(call "$SP1" list_runners)"
L2="$(call "$SP2" list_runners)"
check "the first space lists its own runners" has "$L1" '\\"name\\":\\"hello\\"'
check "and none of the second's" hasnt "$L1" 'only-two'
check "the second space lists its own" has "$L2" 'only-two'
check "and none of the first's" hasnt "$L2" 'hello'

ASK="$(call "$SP1" run_runner '{"name":"restart"}')"
check "a command with a confirm question answers with it and starts nothing" has "$ASK" 'Ends every agent.'
check "no run exists after the refusal" has "$(call "$SP1" list_runs)" '\[\]'

START="$(call "$SP1" run_runner '{"name":"hello"}')"
check "the command starts" has "$START" 'started\\":true'
done_run() { has "$(call "$SP1" list_runs)" 'state\\":\\"exited'; }
check "the run ends within 10 seconds" poll 10 done_run
RUNS="$(call "$SP1" list_runs)"
check "its exit code, 4, is kept" has "$RUNS" 'exit_code\\":4'
LOGPATH="$(printf %s "$RUNS" | sed -n 's/.*log\\":\\"\([^\\]*\)\\".*/\1/p')"
check "its log file is on disk at the path the record names" grep -q hello-from-one "$LOGPATH"
check "the second space sees none of the first's runs" has "$(call "$SP2" list_runs)" '\[\]'
check "the status line shows the failure and the log path" has "$(call "$SP1" get_status)" '✗ hello exit 4'

call "$SP1" run_runner '{"name":"long"}' >/dev/null
running() { has "$(call "$SP1" get_status)" '● long running'; }
check "a long run shows as running in the status line" poll 10 running
ID="$(call "$SP1" list_runs | sed -n 's/.*\\"id\\":\\"\([^\\]*-long-[^\\]*\)\\".*/\1/p')"
RPID="$(pgrep -P "$DPID" -f "$RUNNER serve" | head -1)"
check "the long run's sleep is a child of the runner" test -n "$(pgrep -P "$RPID" -f 'sleep 30')"
call "$SP1" stop_run "{\"id\":\"$ID\"}" >/dev/null
stopped() { has "$(call "$SP1" get_status)" '■ long stopped'; }
check "a stop ends it and the status line says stopped" poll 10 stopped
check "nothing of the stopped run is left running" test -z "$(pgrep -P "$RPID" -f 'sleep 30')"
eq "one wheel-runner process serves both spaces" "$(servers)" 1

# § Runner page (wheel-cmbu.9): the same process draws the page for each space.
page() { local sp="$1"; shift; w artifact call runner-native "$@" --scope machine --space "$sp" 2>&1; }
w artifact read runner-native a2ui://runner-native --scope machine --space "$SP1" >/dev/null 2>&1
P1="$(page "$SP1" get_runner)"
P2="$(page "$SP2" get_runner)"
check "the page lists the first space's commands as Run buttons" has "$P1" 'Run hello'
check "the page of the second space lists its own command" has "$P2" 'Run only-two'
check "and none of the first's" hasnt "$P2" 'Run hello'
check "each run appears in the page's table with its state" has "$P1" 'exit 4'
ASKED="$(page "$SP1" press_run '{"name":"restart"}')"
check "a confirm command's page shows the question and the Run button that names it" has "$ASKED" 'Run restart: Ends every agent.'
check "the question started nothing" test "$(call "$SP1" list_runs | grep -c restart-)" = 0
CANCELLED="$(page "$SP1" cancel_ask)"
check "Cancel puts the plain Run button back" hasnt "$CANCELLED" 'Run restart: Ends every agent.'
page "$SP1" press_run '{"name":"restart","confirmed":true}' >/dev/null
check "the question's Run button starts it" has "$(call "$SP1" list_runs)" 'restart'
LIVE="$(page "$SP1" press_run '{"name":"long"}')"
check "starting a command opens its log, running, with a Stop button" has "$LIVE" 'Stop'
eq "still one wheel-runner process serves every page" "$(servers)" 1
page "$SP1" close_log >/dev/null
finish
