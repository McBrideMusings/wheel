#!/bin/bash
# wheeld on the Linux host, the Mac's second deploy target (ADR-0058), split like the Mac's
# (ADR-0020):
#   linux-server.sh deploy    cross-compile wheeld, wheel and wheel-runner and stage them on the
#                             host as ~/.local/share/<instance>/server/versions/<source hash>/,
#                             never running them
#   linux-server.sh restart   point `current` at the staged version, install the systemd --user
#                             unit when it is missing or changed, restart it, and confirm the new
#                             pid and the source hash `wheel status` reports
# The host is WHEEL_LINUX_HOST (an ssh destination), read from ~/.claude/.env when not already
# set; with none, both steps say so and do nothing. An unreachable host never fails the step: it
# prints the host as behind and exits 0 (a host that answers and then fails a step fails it), so the Mac's deploy and restart that call this still
# succeed. WHEEL_LINUX_INSTANCE (default wheel), WHEEL_LINUX_LISTEN (default 127.0.0.1:7420) and
# WHEEL_LINUX_ARGS (more wheeld arguments) let a check run a throwaway instance beside the real one.
# Output also goes to ~/Library/Logs/wheel/deploy-linux.log.
set -euo pipefail

STEP="${1:-}"
case "$STEP" in
  deploy | restart) ;;
  *) echo "usage: linux-server.sh deploy|restart" >&2; exit 2 ;;
esac
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [ -z "${WHEEL_LINUX_HOST+set}" ] && [ -f "$HOME/.claude/.env" ]; then
  HOST="$(set -a; . "$HOME/.claude/.env"; printf '%s' "${WHEEL_LINUX_HOST:-}")"
else
  HOST="${WHEEL_LINUX_HOST:-}"
fi
INSTANCE="${WHEEL_LINUX_INSTANCE:-wheel}"
LISTEN="${WHEEL_LINUX_LISTEN:-127.0.0.1:7420}"
ARGS="${WHEEL_LINUX_ARGS:-}"
TARGET=x86_64-unknown-linux-gnu
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=10)

LOG_DIR="$HOME/Library/Logs/wheel"
mkdir -p "$LOG_DIR"
exec > >(tee -a "$LOG_DIR/deploy-linux.log") 2>&1
say() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) linux $HOST: $*"; }

if [ -z "$HOST" ]; then
  echo "linux: no host set (WHEEL_LINUX_HOST in ~/.claude/.env); skipped"
  exit 0
fi
# Each remote step is one bash on the host, run as `bash -c <script> bash <args>` with every word
# single-quoted for whatever shell the host logs in with, so stdin stays free for data. It prints
# `key=value` lines this side reads. A host that answered ssh and then failed a step fails it.
shq() { local q=() w; for w in "$@"; do q+=("'${w//\'/\'\\\'\'}'"); done; printf '%s' "${q[*]}"; }
remote() { local script="$1"; shift; "${SSH[@]}" "$HOST" "bash -c $(shq "$script" bash "$INSTANCE" "$@")"; }
fail() { say "$*"; exit 1; }
reach="$("${SSH[@]}" "$HOST" true 2>&1)" || { say "behind: unreachable ($(printf '%s' "$reach" | tail -1))"; exit 0; }

if [ "$STEP" = deploy ]; then
  BUILD_LOG="$LOG_DIR/deploy-linux-build.log"
  say "building wheeld, wheel and wheel-runner for $TARGET; cargo's output is in $BUILD_LOG"
  if ! (cd "$ROOT" && cargo zigbuild --release --target "$TARGET.2.41" -p wheeld -p wheel-cli -p wheel-runner) >"$BUILD_LOG" 2>&1; then
    tail -5 "$BUILD_LOG"
    fail "build failed (see $BUILD_LOG); nothing was staged"
  fi
  # The files arrive as a tar on the same ssh, in a staging folder removed unless it becomes the
  # version. The hash comes from the copied wheeld itself; a version that exists is never touched.
  stage="$(cat <<'EOF'
set -e
root="$HOME/.local/share/$1/server"
mkdir -p "$root/versions"
rm -rf "$root"/versions/.staging.*
staging="$(mktemp -d "$root/versions/.staging.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
tar -xf - -C "$staging"
hash="$("$staging/wheeld" --source-hash)"
[ -e "$root/versions/$hash" ] || mv -T "$staging" "$root/versions/$hash"
ln -sfn "versions/$hash" "$root/staged.new" && mv -T "$root/staged.new" "$root/staged"
echo "staged=$hash"
echo "current=$(basename "$(readlink "$root/current" 2>/dev/null || echo none)")"
echo "pid=$(systemctl --user show -p MainPID --value "$1.service" 2>/dev/null || echo 0)"
EOF
)"
  report="$(tar -C "$ROOT/target/$TARGET/release" -cf - wheeld wheel wheel-runner | remote "$stage")" ||
    fail "staging on the host failed; nothing new was staged"
  staged="$(sed -n 's/^staged=//p' <<<"$report")"
  current="$(sed -n 's/^current=//p' <<<"$report")"
  pid="$(sed -n 's/^pid=//p' <<<"$report")"
  if [ "$staged" = "$current" ]; then
    say "staged $staged, which is running (pid $pid)"
  else
    say "staged $staged; running ${current} (pid $pid), so the host is behind until make restart-server-linux"
  fi
  exit 0
fi

UNIT="$(sed -e "s|@INSTANCE@|$INSTANCE|g" -e "s|@LISTEN@|$LISTEN|g" -e "s|@ARGS@|${ARGS:+ $ARGS}|g" "$ROOT/daemon/linux/wheeld.service.in")"
restart="$(cat <<'EOF'
set -e
root="$HOME/.local/share/$1/server"
unit="$HOME/.config/systemd/user/$1.service"
[ -L "$root/staged" ] || { echo "error=nothing staged in $root; make deploy-linux first"; exit 0; }
if [ "$(loginctl show-user "$USER" -p Linger --value 2>/dev/null)" != yes ]; then
  echo "warning=lingering is off for $USER, so wheeld stops when the last session ends (loginctl enable-linger)"
fi
mkdir -p "$(dirname "$unit")"
if [ "$(cat "$unit" 2>/dev/null)" != "$3" ]; then
  printf '%s\n' "$3" >"$unit.new" && mv -T "$unit.new" "$unit"
  systemctl --user daemon-reload
  echo "unit=installed $unit"
fi
before="$(systemctl --user show -p MainPID --value "$1.service")"
old="$(readlink "$root/current" 2>/dev/null || true)"
new="$(readlink "$root/staged")"
if [ -n "$old" ] && [ "$old" != "$new" ]; then ln -sfn "$old" "$root/previous.new" && mv -T "$root/previous.new" "$root/previous"; fi
ln -sfn "$new" "$root/current.new" && mv -T "$root/current.new" "$root/current"
systemctl --user enable --quiet "$1.service"
systemctl --user restart "$1.service"
echo "before=$before"
echo "version=$(basename "$new")"
# wheeld is up once `wheel status` answers on its listener.
for _ in $(seq 100); do
  if status="$("$root/current/wheel" --url "ws://$2/ws" --token-file "$HOME/.local/share/$1/token" status --json 2>/dev/null)"; then
    echo "after=$(systemctl --user show -p MainPID --value "$1.service")"
    echo "status=$(printf '%s' "$status" | tr -d '\n')"
    exit 0
  fi
  sleep 0.1
done
echo "after=$(systemctl --user show -p MainPID --value "$1.service")"
echo "error=wheel status got no answer on $2 10 s after the restart; journalctl --user -u $1.service"
EOF
)"
report="$(remote "$restart" "$LISTEN" "$UNIT" </dev/null)" || fail "restart failed on the host"
field() { sed -n "s/^$1=//p" <<<"$report"; }
[ -z "$(field warning)" ] || say "warning: $(field warning)"
[ -z "$(field unit)" ] || say "$(field unit)"
[ -z "$(field error)" ] || fail "$(field error)"
source="$(field status | plutil -extract host.source raw -o - - 2>/dev/null || echo none)"
say "restarted $INSTANCE.service: pid $(field before) -> $(field after), version $(field version), status reports source $source"
[ "$source" = "$(field version)" ] || fail "the running wheeld reports source $source, not the version current names"
