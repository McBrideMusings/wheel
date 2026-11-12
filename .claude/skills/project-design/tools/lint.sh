#!/usr/bin/env bash
# lint.sh: DESIGN.md through Google's linter, then every project check DESIGN.md names as
# `(enforced: <check>)`. Exits 1 on any error. Run from anywhere inside the repo.
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
cd "$REPO" || exit 2
status=0

out="$(npx --yes @google/design.md@0.4.0 lint --format json DESIGN.md 2>&1)" || true
errors="$(printf '%s' "$out" | jq -r '.summary.errors // empty' 2>/dev/null)"
if [ -z "$errors" ]; then
  echo "FAIL design.md lint did not run: $out" | head -5; status=1
elif [ "$errors" -gt 0 ]; then
  printf '%s' "$out" | jq -r '.findings[] | select(.severity == "error") | "FAIL design.md: \(.message)"'
  status=1
else
  printf '%s' "$out" | jq -r '.findings[] | select(.severity == "warning") | "warn design.md: \(.message)"'
  echo "ok   design.md lint ($(printf '%s' "$out" | jq -r '.summary.warnings') warnings)"
fi

# Project checks: none yet. A Do's and Don'ts rule marked `(enforced: <check>)` adds its command
# here, in the same change as the rule.

exit $status
