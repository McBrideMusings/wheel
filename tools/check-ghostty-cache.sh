#!/bin/bash
# Fails when two target directories produce two libghostty-vt entries in the shared Zig cache.
# Builds ghostty-vt-sys into two fresh target directories and counts the libraries in the cache
# after each; the second build must add none. Run from anywhere: bash tools/check-ghostty-cache.sh
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
cache="${XDG_CACHE_HOME:-$HOME/.cache}/wheel/ghostty-zig"
work=$(mktemp -d "${TMPDIR:-/tmp}/ghostty-cache-check.XXXXXX")
trap 'rm -rf "$work"' EXIT

count() { { find "$cache/o" -name 'libghostty-vt*.a' 2>/dev/null || true; } | wc -l | tr -d ' '; }
build() {
  local start=$SECONDS
  CARGO_TARGET_DIR="$work/$1" cargo build --manifest-path "$repo/Cargo.toml" -p ghostty-vt-sys >/dev/null 2>&1
  echo "target $1: $(count) libghostty-vt entries in the cache, build took $((SECONDS - start))s"
}

before=$(count)
build a
after_a=$(count)
build b
after_b=$(count)
echo "before: $before, after a: $after_a, after b: $after_b"
if [ "$after_b" != "$after_a" ]; then
  echo "FAIL: a second target directory added $((after_b - after_a)) cache entries" >&2
  exit 1
fi
echo "ok: the second target directory reused the cache"
