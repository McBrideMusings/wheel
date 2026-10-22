#!/usr/bin/env bash
# § 4 Server header: the status word and dot, the details, Stop Server (asks first), Start Server.
# The app stops and starts a server it did not install with the commands in WHEEL_SERVER_STOP and
# WHEEL_SERVER_START; this script gives it commands that signal and start its own wheeld. The
# launch-agent path (SMAppService unregister and register) is for the server the app carries and
# is not run here: it would stop the person's own daemon.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; kill "$(cat "$SCR/daemon.pid" 2>/dev/null)" 2>/dev/null; cleanup' EXIT

TAKEN="$(pick_port)"
python3 - "$TAKEN" <<'PY' &
import socket, sys, time
s = socket.socket(); s.bind(("127.0.0.1", int(sys.argv[1]))); s.listen(); time.sleep(600)
PY
BG_PIDS+=($!)
sleep 0.5

mkdir -p "$SCR/p1"
start_daemon --state-file "$SCR/spaces.json"
echo "$DPID" >"$SCR/daemon.pid"
cat >"$SCR/start.sh" <<EOF
#!/bin/sh
SHELL=/bin/sh nohup "$WHEELD" --listen 127.0.0.1:$PORT --support-dir "$INST" --log-dir "$LOGS" --state-file "$SCR/spaces.json" >>"$SCR/wheeld.out" 2>&1 &
echo \$! >"$SCR/daemon.pid"
EOF
cat >"$SCR/stop.sh" <<EOF
#!/bin/sh
kill "\$(cat "$SCR/daemon.pid")"
EOF
S1="$(w space new "$SCR/p1")"
prefs_reset
launch_app "WHEEL_SERVER_STOP=sh $SCR/stop.sh" "WHEEL_SERVER_START=sh $SCR/start.sh"
wait_st '.connection == "connected" and (.windows[0].spaces | length == 1)' 20
wait_st '.menu_bar.pairing != "Asking wheeld…"' 10

eq "with no remote listener the word says so" "$(st .server.status)" "Running · remote access off"
app_post "/server?action=expand" >/dev/null
check "the details list uptime, spaces and devices" wait_st '.server.details | length == 3' 5
eq "the first detail is the uptime and space count" "$(st '.server.details[0]' | sed 's/up .* · /up … · /')" "wheeld up … · 1 space"
eq "the device line says none are paired" "$(st '.server.details[-1]')" "No paired devices"

# A remote listener that cannot bind: the word, the dot's orange reason in the details.
stop_daemon
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$TAKEN" >"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"
echo "$DPID" >"$SCR/daemon.pid"
check "a listener that cannot bind reads Remote listener failed" wait_st '.server.status == "Remote listener failed"' 20
check "the details carry the daemon's reason" wait_st '.server.details | map(select(startswith("Remote listener failed: "))) | length == 1' 5
# A working listener.
stop_daemon
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$(pick_port)" >"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"
echo "$DPID" >"$SCR/daemon.pid"
check "a working listener reads Running" wait_st '.server.status == "Running"' 20
check "and the details name the address" wait_st '.server.details | map(select(startswith("Listening on "))) | length == 1' 5

# Stop asks first; Cancel changes nothing.
app_post "/server?action=stop" >/dev/null
eq "Stop Server asks before it stops" "$(st .server.confirming)" "true"
eq "nothing has stopped yet" "$(st .server.status)" "Running"
app_post "/server?action=cancel" >/dev/null
eq "Cancel leaves the server running" "$(kill -0 "$(cat "$SCR/daemon.pid")" 2>/dev/null && echo up)" "up"
eq "and the confirmation closes" "$(st .server.confirming)" "false"
expect_exit 22 "the debug endpoint refuses a confirm nobody asked for" curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" "http://127.0.0.1:$DEBUG_PORT/server?action=confirm"

# Confirm: the process ends, and nothing starts it again.
OLD="$(cat "$SCR/daemon.pid")"
app_post "/server?action=stop" >/dev/null
app_post "/server?action=confirm" >/dev/null
check "the header reads Stopped" wait_st '.server.status == "Stopped" and .server.stopped_by_menu' 20
check "the server process is gone" poll 10 sh -c "! kill -0 $OLD 2>/dev/null"
eq "the details say it was stopped from the menu" "$(st '.server.details[0]')" "Stopped from this menu. Terminals and agents in 1 space are not running."
STILL=0
for i in $(seq 1 30); do kill -0 "$OLD" 2>/dev/null && STILL=1; pgrep -f "wheeld --listen 127.0.0.1:$PORT" >/dev/null && STILL=1; sleep 1; done
eq "nothing runs the server again for 30 seconds" "$STILL" "0"

# Start: wheeld comes back and the windows show their spaces again.
app_post "/server?action=start" >/dev/null
check "Start Server brings the server back" wait_st '.server.status == "Running" and (.windows[0].spaces | length == 1)' 30
eq "the stopped flag is cleared" "$(st .server.stopped_by_menu)" "false"
eq "wheeld has its space again" "$(w space ls | jq '.spaces | length')" "1"
finish
