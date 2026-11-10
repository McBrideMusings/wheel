#!/bin/bash
# `wheel-server.sh` keeps the version before Current in Previous and rolls back to it, in a scratch
# HOME with a fake launchctl, so no real launch agent is touched.
#   bash tools/tests/wheel-server-rollback.sh
set -euo pipefail

SRC="$(cd "$(dirname "$0")/../.." && pwd)/apple/Mac/wheel-server.sh"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
export HOME="$T/home"
APPDIR="$T/App/Contents"
mkdir -p "$HOME" "$APPDIR/Resources" "$APPDIR/Helpers" "$T/bin"
cp "$SRC" "$APPDIR/Resources/wheel-server.sh"
PLIST="$APPDIR/Resources/WheelDaemon.plist"
plutil -create xml1 "$PLIST"
plutil -insert Label -string com.test.daemon "$PLIST"
plutil -insert Instance -string wheel-test "$PLIST"
plutil -insert Listen -string 127.0.0.1:1 "$PLIST"
plutil -insert SourceHash -string H2 "$PLIST"
plutil -insert ServerName -string "Wheel Test Server" "$PLIST"
ROOT="$HOME/Library/Application Support/Wheel Test/Server"
mkdir -p "$ROOT/Versions/H1/Wheel Test Server.app" "$ROOT/Versions/H2/Wheel Test Server.app"
ln -s Versions/H1 "$ROOT/Current"

# launchctl: `print` succeeds while $T/loaded exists; every other call is logged.
cat >"$T/bin/launchctl" <<EOF
#!/bin/bash
if [ "\$1" = print ]; then [ -e "$T/loaded" ] && echo "program = x" && exit 0; exit 1; fi
echo "\$*" >>"$T/launchctl.log"
EOF
chmod +x "$T/bin/launchctl"
export PATH="$T/bin:$PATH"
touch "$T/loaded"

S="$APPDIR/Resources/wheel-server.sh"
fail() { echo "FAIL: $*" >&2; exit 1; }
current() { readlink "$ROOT/Current"; }
previous() { readlink "$ROOT/Previous" 2>/dev/null || echo none; }

[ "$(previous)" = none ] || fail "Previous exists before any restart"
bash "$S" rollbackable 2>"$T/why" && fail "rollbackable passed with no Previous"
grep -q "no earlier version" "$T/why" || fail "no reason for a missing Previous: $(cat "$T/why")"

bash "$S" restart >/dev/null
[ "$(current)" = Versions/H2 ] || fail "restart left Current at $(current)"
[ "$(previous)" = Versions/H1 ] || fail "restart left Previous at $(previous)"

bash "$S" restart >/dev/null
[ "$(previous)" = Versions/H1 ] || fail "a restart onto the same version moved Previous to $(previous)"

[ "$(bash "$S" rollbackable)" = H1 ] || fail "rollbackable did not name H1"

: >"$T/launchctl.log"
bash "$S" rollback >/dev/null
[ "$(current)" = Versions/H1 ] || fail "rollback left Current at $(current)"
grep -q "kickstart -k" "$T/launchctl.log" || fail "rollback did not kickstart: $(cat "$T/launchctl.log")"

bash "$S" rollbackable 2>"$T/why" && fail "rollbackable passed when Previous is Current"
grep -q "runs now" "$T/why" || fail "wrong reason: $(cat "$T/why")"

ln -sfn Versions/H2 "$ROOT/Current"
rm "$T/loaded"
bash "$S" rollbackable 2>"$T/why" && fail "rollbackable passed for a stopped agent"
grep -q "stopped" "$T/why" || fail "wrong reason: $(cat "$T/why")"
echo "ok: Previous kept, same-version restart leaves it, rollback moves Current, a stopped agent is refused"
