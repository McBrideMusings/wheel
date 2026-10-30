#!/bin/bash
# Restart the installed app's wheeld from the bundle its launch agent names. This ends every
# terminal and agent session, the pane that ran this one included, so the restart runs detached
# and writes what happened to ~/Library/Logs/wheel/restart-server.log.
#   restart-server.sh release   com.piercemakes.wheel.daemon (Wheel.app)
#   restart-server.sh dev       com.piercemakes.wheel.dev.daemon (Wheel Dev.app)
set -euo pipefail

case "${1:-}" in
  release) LABEL=com.piercemakes.wheel.daemon ;;
  dev) LABEL=com.piercemakes.wheel.dev.daemon ;;
  --detached) LABEL="$2" ;;
  *) echo "usage: restart-server.sh release|dev" >&2; exit 2 ;;
esac
SERVICE="gui/$(id -u)/$LABEL"
LOG_DIR="$HOME/Library/Logs/wheel"
LOG="$LOG_DIR/restart-server.log"
say() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*"; }
pid_of() { launchctl print "$SERVICE" 2>/dev/null | awk '$1 == "pid" { print $3; exit }'; }

if [ "${1:-}" != --detached ]; then
  launchctl print "$SERVICE" >/dev/null 2>&1 || { echo "$LABEL is not loaded; open the app that runs it first" >&2; exit 1; }
  mkdir -p "$LOG_DIR"
  nohup bash "$0" --detached "$LABEL" >>"$LOG" 2>&1 </dev/null &
  echo "restarting $LABEL (pid $(pid_of)); every terminal and agent session ends. Log: $LOG"
  exit 0
fi

# Give the message above a moment to reach the pane before the restart closes it.
sleep 1
before="$(pid_of)"
say "restarting $SERVICE (pid ${before:-none})"
launchctl kickstart -k "$SERVICE"
for _ in $(seq 50); do
  after="$(pid_of)"
  if [ -n "$after" ] && [ "$after" != "$before" ]; then
    say "restarted $SERVICE: pid ${before:-none} -> $after, running $(lsof -a -p "$after" -d txt -Fn 2>/dev/null | sed -n 's/^n//p' | head -1)"
    exit 0
  fi
  sleep 0.1
done
say "$SERVICE has no new pid 5 s after the restart (was ${before:-none}, now ${after:-none})"
exit 1
