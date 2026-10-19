#!/usr/bin/env bash
# Shared helpers for the verify-project check scripts. Source it; never run it.
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
  local p
  for p in "${BG_PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null; done
  [ -n "$DPID" ] && kill "$DPID" 2>/dev/null && wait "$DPID" 2>/dev/null
  rm -rf "$SCR"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# The CLI against this script's daemon.
w() { "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$INST/token" "$@"; }
TOKEN() { cat "$INST/token"; }
BASE() { echo "http://127.0.0.1:$PORT"; }
LOG() { echo "$LOGS/wheeld.log"; }

wait_port() {
  local i
  for i in $(seq 1 150); do
    ! port_free "$PORT" && return 0
    sleep 0.1
  done
  return 1
}

# start_daemon [wheeld args...]: a throwaway daemon with its own support and log directories.
# Remembers the arguments so restart_daemon can bring it back on the same state.
start_daemon() {
  DARGS=("$@")
  [ -n "$PORT" ] || PORT="$(pick_port)"
  SHELL=/bin/sh "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$INST" --log-dir "$LOGS" "${DARGS[@]}" \
    >>"$SCR/wheeld.out" 2>&1 &
  DPID=$!
  wait_port || { echo "wheeld did not bind $PORT; see $SCR/wheeld.out" >&2; cat "$SCR/wheeld.out" >&2; exit 2; }
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
  local end=$((SECONDS + $1 + 1)) n=$(($1 * 10)) i; shift
  for i in $(seq 1 "$n"); do "$@" >/dev/null 2>&1 && return 0; sleep 0.1; done
  return 1
}

# The lead's MCP endpoint. mcp <tool> [<json arguments>] prints the JSON-RPC reply.
mcp() {
  curl -m "${MCP_TIMEOUT:-20}" -s -X POST -H "Authorization: Bearer $(TOKEN)" -H 'Content-Type: application/json' \
    -H 'Accept: application/json, text/event-stream' "$(BASE)/mcp" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":${2:-{\}}}}"
}
# The first text block of a tool reply, and whether it was an error ("true"/"false").
mcp_text() { jget result.content.0.text; }
mcp_err() { jget result.isError; }

# perm_id: the id of the first waiting permission request, or nothing.
perm_id() { w perm list 2>/dev/null | jget 0.id; }

# mcp_asked <choice> <tool> [<json arguments>]: a tool that raises a permission request. Calls it in the
# background, answers the request with <choice> (allow|deny), prints the tool's reply.
mcp_asked() {
  local choice="$1" out="$SCR/mcp-asked.$RANDOM"; shift
  mcp "$@" >"$out" &
  local cp=$!
  BG_PIDS+=("$cp")
  if poll 15 perm_id; then w perm answer "$(perm_id)" "$choice" >/dev/null 2>&1; fi
  wait "$cp" 2>/dev/null
  cat "$out"
}

finish() {
  echo "$PASS passed, $FAIL failed"
  [ "$FAIL" = 0 ]
  exit $?
}
