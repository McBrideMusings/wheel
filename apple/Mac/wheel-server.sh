#!/bin/bash
# Install and run the Wheel server this app carries. The app ships the server bundle as
# Contents/Helpers/<name>.wheelserver (not .app, so macOS does not treat it as a second app inside
# Wheel.app); this script copies it out of the app to
#   ~/Library/Application Support/<Wheel|Wheel Dev>/Server/Versions/<source hash>/<name>.app
# and runs that copy as its own launchd agent. macOS charges folder access to the program that
# asks, and a server inside Wheel.app is charged to Wheel; a copy outside it is Wheel Server.
#
#   wheel-server.sh start     copy this app's version if it is not there yet, and load the agent if
#                             launchd has no job for it. A job that is running is left alone
#                             (ADR-0020: a client deploy never restarts the server).
#   wheel-server.sh restart   make this app's version Current and run it: the owner's step, it ends
#                             every terminal and agent.
#   wheel-server.sh stop      unload the agent and keep it from loading at login.
#
# The agent's plist is ~/Library/LaunchAgents/<label>.plist, so launchd loads it at login with no
# app running, as the login item did before. It names .../Server/Current/<name>.app, a symlink that
# only `restart` moves, so a crash restart by KeepAlive never starts a version the owner has not
# asked for. A version folder is written once and never changed, so a running server's files stay.
set -euo pipefail

RES="$(cd "$(dirname "$0")" && pwd)"
APP="$(cd "$RES/../.." && pwd)"
field() { plutil -extract "$1" raw -o - "$RES/WheelDaemon.plist"; }
LABEL="$(field Label)"
INSTANCE="$(field Instance)"
LISTEN="$(field Listen)"
HASH="$(field SourceHash)"
NAME="$(field ServerName)"
ROOT="$HOME/Library/Application Support/${NAME% Server}/Server"
VERSION="$ROOT/Versions/$HASH"
CURRENT="$ROOT/Current"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
DOMAIN="gui/$(id -u)"
say() { echo "$*"; }

loaded() { launchctl print "$DOMAIN/$LABEL" >/dev/null 2>&1; }

install_version() {
  [ -d "$VERSION/$NAME.app" ] && return 0
  mkdir -p "$ROOT/Versions"
  local stage="$ROOT/Versions/.staging-$$"
  rm -rf "$ROOT"/Versions/.staging-*
  mkdir -p "$stage"
  ditto "$APP/Contents/Helpers/$NAME.wheelserver" "$stage/$NAME.app"
  codesign --verify --strict "$stage/$NAME.app"
  # A folder that exists without the app is a failed earlier copy; a complete one is another run's.
  if [ -e "$VERSION" ] && [ ! -d "$VERSION/$NAME.app" ]; then rm -rf "$VERSION"; fi
  if [ -e "$VERSION" ]; then
    rm -rf "$stage"
  else
    mv "$stage" "$VERSION"
  fi
  [ -d "$VERSION/$NAME.app" ]
  say "copied the server ($HASH) to $VERSION"
}

# Moves Current to this version in one rename, so there is no moment without a Current.
point_current_here() {
  ln -sfn "Versions/$HASH" "$ROOT/.Current.new"
  mv -fh "$ROOT/.Current.new" "$CURRENT"
}

# Returns 0 when the plist on disk changed.
write_plist() {
  local next="$PLIST.new"
  mkdir -p "$(dirname "$PLIST")" "$HOME/Library/Logs/$INSTANCE"
  rm -f "$next"
  plutil -create xml1 "$next"
  plutil -insert Label -string "$LABEL" "$next"
  local args
  args="$(printf '["%s","--instance","%s","--listen","%s","--cli","%s","--path-from-login-shell"]' \
    "$CURRENT/$NAME.app/Contents/MacOS/wheeld" "$INSTANCE" "$LISTEN" "$APP/Contents/Helpers/wheel")"
  plutil -insert ProgramArguments -json "$args" "$next"
  plutil -insert RunAtLoad -bool true "$next"
  plutil -insert KeepAlive -bool true "$next"
  plutil -insert StandardOutPath -string "$HOME/Library/Logs/$INSTANCE/launchd.log" "$next"
  plutil -insert StandardErrorPath -string "$HOME/Library/Logs/$INSTANCE/launchd.log" "$next"
  if cmp -s "$next" "$PLIST" 2>/dev/null; then rm -f "$next"; return 1; fi
  mv -f "$next" "$PLIST"
}

bootstrap() {
  launchctl enable "$DOMAIN/$LABEL"
  launchctl bootstrap "$DOMAIN" "$PLIST"
}

case "${1:-}" in
  start)
    install_version
    # Written even when a job runs (one the earlier app registered from inside its bundle, whose
    # program this build deleted), so a crash or the next login starts the copy. The running
    # process is not touched.
    [ -e "$CURRENT" ] || point_current_here
    write_plist || true
    if loaded; then
      say "$LABEL is running; left alone"
      exit 0
    fi
    bootstrap
    say "$LABEL loaded, running $(readlink "$CURRENT")"
    ;;
  restart)
    install_version
    point_current_here
    changed=0
    write_plist && changed=1
    # A job started from inside an app bundle (the earlier arrangement) or from a plist that has
    # since changed is replaced; launchd keeps the definition it was given.
    if loaded; then
      description="$(launchctl print "$DOMAIN/$LABEL")"
      if [ "$changed" = 1 ] || grep -q 'parent bundle identifier\|program = .*/Contents/Helpers/' <<<"$description"; then
        launchctl bootout "$DOMAIN/$LABEL"
        for _ in $(seq 50); do loaded || break; sleep 0.1; done
      fi
    fi
    if loaded; then
      launchctl kickstart -k "$DOMAIN/$LABEL"
      say "$LABEL restarted, running $HASH"
    else
      bootstrap
      say "$LABEL loaded, running $HASH"
    fi
    ;;
  stop)
    launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
    launchctl disable "$DOMAIN/$LABEL"
    say "$LABEL stopped and disabled"
    ;;
  *)
    echo "usage: wheel-server.sh start|restart|stop" >&2
    exit 2
    ;;
esac
