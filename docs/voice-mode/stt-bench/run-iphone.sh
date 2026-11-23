#!/bin/bash
# Runs each spec twice on the attached iPhone (unlocked, screen on): pass 1 downloads and compiles, pass 2 measures. Pulls JSON into results/iphone.
set -uo pipefail
B=$(cd "$(dirname "$0")" && pwd)
DEV=00008140-001E1CEE1E81801C
APP=com.piercemakes.sttbench
mkdir -p "$B/results/iphone"
specs=("$@")
[ ${#specs[@]} -eq 0 ] && specs=(fluid:v3 fluid:v2 whisperkit:openai_whisper-small.en_217MB whisperkit:openai_whisper-large-v3-v20240930_turbo_632MB)
for s in "${specs[@]}"; do
  f=$(echo "$s" | tr ':/' '__')
  for pass in pass1 pass2; do
    echo "== $s $pass $(date -u +%H:%M:%S)"
    xcrun devicectl device process launch --device "$DEV" --terminate-existing "$APP" -- -spec "$s" -tag "$pass" >/dev/null 2>&1 || { echo "LAUNCH FAILED (locked?)"; exit 1; }
    for _ in $(seq 1 180); do
      sleep 10
      xcrun devicectl device copy from --device "$DEV" --domain-type appDataContainer --domain-identifier "$APP" \
        --source "Documents/$f.$pass.log" --destination "$B/results/iphone/$f.$pass.log" >/dev/null 2>&1 || continue
      if grep -q -E "^(DONE|ERROR)" "$B/results/iphone/$f.$pass.log"; then break; fi
    done
    grep -E "loaded|DONE|ERROR|battery" "$B/results/iphone/$f.$pass.log"
    xcrun devicectl device copy from --device "$DEV" --domain-type appDataContainer --domain-identifier "$APP" \
      --source "Documents/$f.$pass.json" --destination "$B/results/iphone/$f.$pass.json" >/dev/null 2>&1 || echo "no json for $s $pass"
  done
done
echo "== done $(date -u +%H:%M:%S)"
