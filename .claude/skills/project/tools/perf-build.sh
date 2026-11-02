#!/bin/bash
# perf.md § build: a test run and a build that follows it recompile nothing a build before them
# already compiled. Prints the crates each step compiled; exits non-zero over the limit.
set -u
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
cd "$REPO" || exit 2
LOG="$(mktemp "${TMPDIR:-/tmp}/wheel-perf-build.XXXXXX")"
trap 'rm -f "$LOG"' EXIT
compiled() { "$@" >"$LOG" 2>&1 || { tail -20 "$LOG" >&2; return 2; }; grep -c ' Compiling ' "$LOG" || true; }

# The test target and its dev-dependencies compile on their first run; build both once first.
compiled cargo test -p wheel-cli --test cli space_status >/dev/null || exit 2
compiled cargo build >/dev/null || exit 2
test_run=$(compiled cargo test -p wheel-cli --test cli space_status) || exit 2
build_after=$(compiled cargo build) || exit 2
echo "crates compiled by cargo test -p wheel-cli after cargo build: $test_run (limit 0)"
echo "crates compiled by cargo build after that test run: $build_after (limit 0)"
[ "$test_run" -eq 0 ] && [ "$build_after" -eq 0 ]
