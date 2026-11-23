#!/bin/bash
# Final Mac pass: every engine once more with models cached, plus whisper.cpp through whisper-server; logs the load average before each.
set -uo pipefail
B=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$B/results/mac-final"
for s in fluid:v3 fluid:v2 whisperkit:openai_whisper-small.en_217MB whisperkit:openai_whisper-large-v3-v20240930_turbo_632MB; do
  f=$(echo "$s" | tr ':/' '__')
  echo "== $s $(date -u +%H:%M:%S) $(sysctl -n vm.loadavg)"
  "$B/Bench/.build/release/bench" "$s" "$B/fixture" "$B/models" "$B/results/mac-final/$f.json" 2>&1 | grep -E "loaded|wrote|rror"
done
for m in small.en large-v3-turbo-q5_0; do
  echo "== whisper.cpp $m $(date -u +%H:%M:%S) $(sysctl -n vm.loadavg)"
  python3 -I "$B/run-whispercpp-mac.py" "$B/models/ggml-$m.bin" "$B/results/mac-final/whispercpp_$m.json" 2>&1 | tail -1
done
echo "== done $(date -u +%H:%M:%S)"
