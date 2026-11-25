#!/bin/bash
# Drives tools/linux-server.sh against the Linux host (WHEEL_LINUX_HOST, from ~/.claude/.env) with
# a throwaway instance, wheel-check on 127.0.0.1:7491, beside whatever the host runs, and removes
# it afterwards:
#   1. deploy stages a version and leaves the running pid alone
#   2. restart changes the pid and `wheel status` reports the staged source hash
#   3. a second deploy of the same build keeps one version and clears a stale staging folder, and
#      its restart restarts again
#   4. with ssh refused, deploy and restart exit 0 and print the host as behind
#   bash tools/linux-server-check.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
S="$ROOT/tools/linux-server.sh"
[ -f "$HOME/.claude/.env" ] && { set -a; . "$HOME/.claude/.env"; set +a; }
HOST="${WHEEL_LINUX_HOST:?set WHEEL_LINUX_HOST}"
export WHEEL_LINUX_INSTANCE=wheel-check WHEEL_LINUX_LISTEN=127.0.0.1:7491
export WHEEL_LINUX_ARGS="--machine-file %h/.local/share/wheel-check/wheel.toml"
on_host() { ssh -o BatchMode=yes "$HOST" "$@"; }
cleanup() {
  on_host 'systemctl --user disable --now wheel-check.service 2>/dev/null; rm -f ~/.config/systemd/user/wheel-check.service; systemctl --user daemon-reload; rm -rf ~/.local/share/wheel-check ~/.local/state/wheel-check' || true
}
trap cleanup EXIT
cleanup
fail() { echo "FAIL: $*" >&2; exit 1; }
pid() { on_host systemctl --user show -p MainPID --value wheel-check.service; }

out="$(bash "$S" deploy)"; echo "$out"
hash="$(on_host 'basename "$(readlink ~/.local/share/wheel-check/server/staged)"')"
[ -n "$hash" ] || fail "1: deploy staged nothing"
grep -q "staged $hash; running none" <<<"$out" || fail "1: deploy report"
[ "$(pid)" = 0 ] || fail "1: deploy started wheeld"
echo "ok 1: staged $hash, nothing running"

out="$(bash "$S" restart)"; echo "$out"
first="$(pid)"
[ "$first" != 0 ] || fail "2: no wheeld after the restart"
grep -q "pid 0 -> $first, version $hash, status reports source $hash" <<<"$out" || fail "2: restart report"
echo "ok 2: pid $first, status reports $hash"

on_host 'mkdir ~/.local/share/wheel-check/server/versions/.staging.stale'
out="$(bash "$S" deploy)"; echo "$out"
grep -q "staged $hash, which is running (pid $first)" <<<"$out" || fail "3: second deploy report"
[ "$(pid)" = "$first" ] || fail "3: deploy changed the pid"
[ "$(on_host 'ls -A ~/.local/share/wheel-check/server/versions | wc -l')" = 1 ] || fail "3: more than one version, or a staging folder left"
out="$(bash "$S" restart)"; echo "$out"
[ "$(pid)" != "$first" ] || fail "3: restart kept the pid"
echo "ok 3: one version, pid $first -> $(pid)"

for step in deploy restart; do
  out="$(WHEEL_LINUX_HOST=ssh://nobody@127.0.0.1:1 bash "$S" "$step")" || fail "4: $step exited $?"
  echo "$out"
  grep -q "behind: unreachable" <<<"$out" || fail "4: $step did not say behind"
done
echo "ok 4: refused ssh exits 0 and prints behind"
