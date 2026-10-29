#!/bin/bash
# Checks sessions.sh against a fixture roots directory: two transcripts written now with different
# cwd, one written 60 s ago. Run: bash artifacts/companion/sessions.test.sh
here="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
pass=0; failed=0
eq() { if [ "$2" = "$3" ]; then pass=$((pass + 1)); echo "ok   $1"; else failed=$((failed + 1)); echo "FAIL $1: got $2, want $3"; fi; }

mkdir -p "$tmp/roots/a" "$tmp/roots/b" "$tmp/data"
printf '{"type":"summary"}\n{"cwd":"/Users/x/Projects/alpha","type":"user"}\n' >"$tmp/roots/a/s-alpha.jsonl"
printf '{"cwd":"/Users/x/Projects/beta","type":"user"}\n' >"$tmp/roots/b/s-beta.jsonl"
printf '{"cwd":"/Users/x/Projects/old","type":"user"}\n' >"$tmp/roots/b/s-old.jsonl"
touch -t "$(date -v-60S +%Y%m%d%H%M.%S)" "$tmp/roots/b/s-old.jsonl"
printf '{"source":"all","roots":["%s"],"max":8}\n' "$tmp/roots" >"$tmp/config.json"

run() { WHEEL_DATA_DIR="$tmp/data" COMPANION_CONFIG="$tmp/config.json" bash "$here/sessions.sh"; }

run
eq "all: lists exactly the two recent transcripts" "$(jq -c '[.sessions[] | [.id, .project, .state]] | sort' "$tmp/data/sessions.json")" \
  '[["s-alpha","/Users/x/Projects/alpha","working"],["s-beta","/Users/x/Projects/beta","working"]]'
eq "all: no error" "$(jq -r '.error // "none"' "$tmp/data/sessions.json")" none

printf '{"source":"all","roots":["%s"],"max":1}\n' "$tmp/roots" >"$tmp/config.json"
run
eq "max caps the list" "$(jq '.sessions | length' "$tmp/data/sessions.json")" 1

printf '{"source":"session:s-beta","roots":["%s"]}\n' "$tmp/roots" >"$tmp/config.json"
WHEEL_BIN=/nonexistent run
eq "session:<id> on a transcript lists only it" "$(jq -r '[.sessions[].id] | join(",")' "$tmp/data/sessions.json")" s-beta

printf '{"source":"nonsense"}\n' >"$tmp/config.json"
run
eq "an unknown source is reported" "$(jq -r '.error' "$tmp/data/sessions.json")" "source nonsense is not all, wheel or session:<id>"

echo "$pass passed, $failed failed"
[ "$failed" = 0 ]
