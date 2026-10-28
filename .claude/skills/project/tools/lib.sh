#!/usr/bin/env bash
# Shared helpers for the project skill's check scripts (verify.md). Source it; never run it.
#
# A script sources this file, calls start_daemon, runs its checks with ok/fail/check/
# expect_exit, and ends with `finish`. Everything the script creates lives in one mktemp
# directory ($SCR) and one daemon port, both removed or freed on exit, so two scripts
# (different $SWARM_WORKER, or none) run side by side.

CHECKS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$CHECKS_DIR/../../../.." && pwd)"
BIN="${CARGO_TARGET_DIR:-$REPO/target}/debug"
WHEELD="$BIN/wheeld"
WHEEL="$BIN/wheel"
FAKE_AGENT="$BIN/wheel-fake-agent"

for c in plutil curl perl pgrep git; do
  command -v "$c" >/dev/null || { echo "these checks need $c (macOS)" >&2; exit 2; }
done
for f in "$WHEELD" "$WHEEL" "$FAKE_AGENT"; do
  [ -x "$f" ] || { echo "missing $f: run cargo build in $REPO first" >&2; exit 2; }
done

SCR="$(mktemp -d "${TMPDIR:-/tmp}/wheel-check.XXXXXX")"
SCR="$(cd "$SCR" && pwd -P)"
INST="$SCR/inst"
LOGS="$SCR/logs"
mkdir -p "$INST" "$LOGS"
PASS=0
FAIL=0
DPID=""
DARGS=()
BG_PIDS=()
PORT=""

port_free() { ! (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; }

# A port nothing listens on: starts at a number derived from $SWARM_WORKER (or random), walks up.
pick_port() {
  local p
  if [ -n "${SWARM_WORKER:-}" ]; then
    p=$((7500 + $(printf %s "$SWARM_WORKER" | cksum | cut -d' ' -f1) % 400))
  else
    p=$((20000 + RANDOM % 20000))
  fi
  while ! port_free "$p"; do p=$((p + 1)); done
  echo "$p"
}

cleanup() {
  local p i
  for p in "${BG_PIDS[@]:-}"; do
    [ -n "$p" ] || continue
    pkill -P "$p" 2>/dev/null
    kill "$p" 2>/dev/null
  done
  if [ -n "$DPID" ]; then
    kill "$DPID" 2>/dev/null
    for i in $(seq 1 30); do kill -0 "$DPID" 2>/dev/null || break; sleep 0.1; done
    kill -9 "$DPID" 2>/dev/null
    wait "$DPID" 2>/dev/null
  fi
  rm -rf "$SCR"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# The CLI against this script's daemon.
w() { "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$INST/token" "$@"; }
TOKEN() { cat "$INST/token"; }
BASE() { echo "http://127.0.0.1:$PORT"; }
LOG() { echo "$LOGS/wheeld.log"; }

# wait_port: the port answers while the daemon we started is still alive (a daemon that lost the
# bind to another script exits, and the port then answers for that other script's daemon).
wait_port() {
  local i
  for i in $(seq 1 150); do
    kill -0 "$DPID" 2>/dev/null || return 1
    ! port_free "$PORT" && return 0
    sleep 0.1
  done
  return 1
}

# use_fake_agent: before start_daemon, point the claude agent at wheel-fake-agent, in the machine
# wheel.toml of this script's support directory.
use_fake_agent() {
  printf '[agent.claude]\ncommand = "%s"\n' "$FAKE_AGENT" >"$INST/wheel.toml"
}

# start_daemon [wheeld args...]: a throwaway daemon with its own support and log directories.
# Remembers the arguments so restart_daemon can bring it back on the same state. When the bind
# fails because another script took the port first, it tries the next ports.
start_daemon() {
  DARGS=("$@")
  local try
  for try in 1 2 3 4 5 6; do
    [ -n "$PORT" ] || PORT="$(pick_port)"
    SHELL=/bin/sh "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$INST" --machine-file "$INST/wheel.toml" --log-dir "$LOGS" "${DARGS[@]}" \
      >>"$SCR/wheeld.out" 2>&1 &
    DPID=$!
    wait_port && return 0
    wait "$DPID" 2>/dev/null
    DPID=""
    PORT=""
    sleep 0.2
  done
  echo "wheeld did not start; see $SCR/wheeld.out" >&2; cat "$SCR/wheeld.out" >&2; exit 2
}

stop_daemon() {
  [ -n "$DPID" ] || return 0
  kill "$DPID" 2>/dev/null
  wait "$DPID" 2>/dev/null
  DPID=""
}

restart_daemon() {
  stop_daemon
  start_daemon "${DARGS[@]}"
}

ok() { PASS=$((PASS + 1)); echo "ok   $1"; }
fail() { FAIL=$((FAIL + 1)); echo "FAIL $1"; [ -n "${2:-}" ] && echo "     $2" | head -5; return 0; }

# check <desc> <cmd...>: passes when the command exits 0.
check() {
  local d="$1" out; shift
  if out="$("$@" 2>&1)"; then ok "$d"; else fail "$d" "$out"; fi
}

# expect_exit <code> <desc> <cmd...>
expect_exit() {
  local want="$1" d="$2" out got; shift 2
  out="$("$@" 2>&1)"; got=$?
  if [ "$got" = "$want" ]; then ok "$d"; else fail "$d" "exit $got, wanted $want: $out"; fi
}

# expect_out <pattern> <desc> <cmd...>: output matches the extended regex, whatever the exit code.
expect_out() {
  local pat="$1" d="$2" out; shift 2
  out="$("$@" 2>&1)"
  if printf %s "$out" | grep -Eq -- "$pat"; then ok "$d"; else fail "$d" "no match for /$pat/ in: $out"; fi
}

# expect_exit_out <code> <pattern> <desc> <cmd...>: both the exit code and the output.
expect_exit_out() {
  local want="$1" pat="$2" d="$3" out got; shift 3
  out="$("$@" 2>&1)"; got=$?
  if [ "$got" = "$want" ] && printf %s "$out" | grep -Eq -- "$pat"; then ok "$d"
  else fail "$d" "exit $got (wanted $want), /$pat/ in: $out"; fi
}

# file_lacks <pattern> <file>: the file exists and has no line matching the pattern (a missing
# file is not a pass). Use with check: check "desc" file_lacks pattern file
file_lacks() { [ -f "$2" ] && ! grep -q -- "$1" "$2"; }

# lacks <desc> <key> <cmd...>: the command succeeds, prints JSON, and that JSON has no "key" field
# (so a failed command or an empty reply is not read as "absent").
lacks() {
  local d="$1" key="$2" out; shift 2
  if out="$("$@" 2>&1)" && [ -n "$out" ] && ! printf %s "$out" | grep -q "\"$key\""; then ok "$d"; else fail "$d" "$out"; fi
}

# eq <desc> <got> <want>
eq() { if [ "$2" = "$3" ]; then ok "$1"; else fail "$1" "got '$2', wanted '$3'"; fi; }

# has_log <pattern> [desc]: the daemon log holds a line matching the extended regex.
has_log() {
  if grep -Eq -- "$1" "$(LOG)" 2>/dev/null; then ok "${2:-log has $1}"; else fail "${2:-log has $1}" "not in $(LOG)"; fi
}

# jget <keypath>: read one value from JSON on stdin with macOS plutil (a.b.0.c; arrays by index).
jget() { plutil -extract "$1" raw -o - - 2>/dev/null; }

# bg <cmd...>: run in the background, remember the pid so cleanup stops it. Sets BGPID.
bg() { "$@" & BGPID=$!; BG_PIDS+=("$BGPID"); }

# poll <seconds> <cmd...>: run the command every 100 ms until it exits 0 or the time passes.
poll() {
  local n=$(($1 * 10)) i; shift
  for i in $(seq 1 "$n"); do "$@" >/dev/null 2>&1 && return 0; sleep 0.1; done
  return 1
}

# perm_id: the id of the first waiting permission request, or nothing.
perm_id() { w perm list 2>/dev/null | jget 0.id; }

finish() {
  echo "$PASS passed, $FAIL failed"
  [ "$FAIL" = 0 ]
  exit $?
}
