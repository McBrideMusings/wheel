#!/bin/bash
# Runs each engine spec twice on the Mac, each in its own process: pass 1 downloads and compiles, pass 2 is the measured run.
set -uo pipefail
B=$(cd "$(dirname "$0")" && pwd)
swift build -c release --package-path "$B/Bench" 2>&1 | tail -1
mkdir -p "$B/results/mac"
specs=("$@")
[ ${#specs[@]} -eq 0 ] && specs=(fluid:v2 fluid:v3 whisperkit:openai_whisper-small.en_217MB whisperkit:openai_whisper-large-v3-v20240930_turbo_632MB)
for s in "${specs[@]}"; do
  f=$(echo "$s" | tr ':/' '__')
  for pass in 1 2; do
    echo "== $s pass $pass $(date -u +%H:%M:%S)"
    "$B/Bench/.build/release/bench" "$s" "$B/fixture" "$B/models" "$B/results/mac/$f.pass$pass.json" 2>&1 | grep -E "loaded|wrote|rror" || echo "FAILED $s pass $pass"
  done
done
echo "== done $(date -u +%H:%M:%S)"
