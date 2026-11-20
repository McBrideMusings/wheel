#!/bin/bash
# project-perf § build: a test run and a build that follows it recompile nothing a build before them
# already compiled, a build of one package reuses the workspace build's artifacts, and the whole
# run writes one copy of libwheeld and of libclap. Prints each measure; exits non-zero over a limit.
set -u
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
cd "$REPO" || exit 2
LOG="$(mktemp "${TMPDIR:-/tmp}/wheel-perf-build.XXXXXX")"
START="$(mktemp "${TMPDIR:-/tmp}/wheel-perf-build-start.XXXXXX")"
trap 'rm -f "$LOG" "$START"' EXIT
DEPS="${CARGO_TARGET_DIR:-$REPO/target}/debug/deps"
compiled() { "$@" >"$LOG" 2>&1 || { tail -20 "$LOG" >&2; return 2; }; grep -c ' Compiling ' "$LOG" || true; }
# rlibs of a crate this run wrote: a second one is a second feature set.
copies() { find "$DEPS" -name "lib$1-*.rlib" -newer "$START" 2>/dev/null | wc -l | tr -d ' '; }

# The test target and its dev-dependencies compile on their first run; build both once first.
compiled cargo test -p wheel-cli --test cli space_status >/dev/null || exit 2
compiled cargo build >/dev/null || exit 2
test_run=$(compiled cargo test -p wheel-cli --test cli space_status) || exit 2
build_after=$(compiled cargo build) || exit 2
wheelhouse=$(compiled cargo build -p wheelhouse) || exit 2
wheeld=$(compiled cargo build -p wheeld) || exit 2
wheeld_copies=$(copies wheeld)
clap_copies=$(copies clap)
echo "crates compiled by cargo test -p wheel-cli after cargo build: $test_run (limit 0)"
echo "crates compiled by cargo build after that test run: $build_after (limit 0)"
echo "crates compiled by cargo build -p wheelhouse after that: $wheelhouse (limit 0)"
echo "crates compiled by cargo build -p wheeld after that: $wheeld (limit 0)"
echo "libwheeld rlibs this run wrote: $wheeld_copies (limit 1)"
echo "libclap rlibs this run wrote: $clap_copies (limit 1)"
[ "$test_run" -eq 0 ] && [ "$build_after" -eq 0 ] && [ "$wheelhouse" -eq 0 ] && [ "$wheeld" -eq 0 ] \
  && [ "$wheeld_copies" -le 1 ] && [ "$clap_copies" -le 1 ]
