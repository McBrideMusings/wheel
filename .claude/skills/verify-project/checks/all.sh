#!/usr/bin/env bash
# Runs every check script in turn and prints each one's last line. Exits non-zero if any failed.
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
failed=0
for s in daemon spaces web browser settings lead-controls lead-mcp persons inbox supervisors usage; do
  printf '%-12s ' "$s"
  out="$(bash "$here/$s.sh" 2>&1)"; code=$?
  printf '%s\n' "$(printf '%s\n' "$out" | tail -1)"
  if [ "$code" != 0 ]; then
    failed=$((failed + 1))
    printf '%s\n' "$out" | grep '^FAIL' | sed 's/^/    /'
  fi
done
[ "$failed" = 0 ]
