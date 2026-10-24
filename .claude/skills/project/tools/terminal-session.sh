#!/bin/bash
# Scripted /clear: a stand-in claude (a node script, so the daemon detects it) runs the SessionStart
# hook command with a new session id; the pane must save it, and a view switch must keep it.
set -u
T=$CARGO_TARGET_DIR/debug
SCR=$(mktemp -d)
PORT=$((7500 + $(printf %s "${SWARM_WORKER:-x}" | cksum | cut -d" " -f1) % 400))
mkdir -p $SCR/run/inst $SCR/run/logs $SCR/run/bin $SCR/run/cfg $SCR/run/space
cd $SCR/run
A=11111111-2222-3333-4444-555555555555
B=66666666-7777-8888-9999-000000000000
cat > bin/claude <<JS
#!/usr/bin/env node
const { execSync } = require('child_process');
console.log('stand-in claude ready');
setTimeout(() => {
  // what the SessionStart hook runs after /clear, through a shell like Claude Code does
  const out = execSync('sh -c "$T/wheel pane report-session --session $B; echo hook-exit=\$?"', {env: process.env}).toString();
  console.log(out);
}, 4000);
setInterval(() => {}, 1000);
JS
chmod +x bin/claude
cat > space/wheel.toml <<TOML
[terminal]
shell = "$SCR/run/shell.sh"
TOML
cat > shell.sh <<SH
#!/bin/sh
export PATH=$SCR/run/bin:\$PATH CLAUDE_CONFIG_DIR=$SCR/run/cfg WHEEL_URL=ws://127.0.0.1:$PORT/ws WHEEL_TOKEN_FILE=$SCR/run/inst/token
exec /bin/sh -i
SH
chmod +x shell.sh
$T/wheeld --listen 127.0.0.1:$PORT --support-dir inst --log-dir logs >daemon.out 2>&1 &
D=$!
trap 'kill $D 2>/dev/null; rm -rf $SCR' EXIT
sleep 3
export WHEEL_URL=ws://127.0.0.1:$PORT/ws WHEEL_TOKEN_FILE=$SCR/run/inst/token
unset WHEEL_PANE
W=$T/wheel
$W space new $SCR/run/space
$W space ls > ls.json
P=$(python3 -I -c 'import json;d=json.load(open("ls.json"));print(d["spaces"][0]["focus"])')
$W pane kind $P terminal
echo pane=$P
echo "-- a report from outside any Claude Code is refused:"
WHEEL_PANE=$P $W pane report-session --session $B; echo "exit=$?"
$W term send $P "claude --session-id $A" --enter
sleep 3
echo "view before the hook (want $A): $($W pane view $P)"
sleep 6
echo "term screen:"; $W term read $P --timeout 0.3 | tail -5
echo "view after the hook (want $B): $($W pane view $P)"
grep -h 'reported its session\|session report ignored' logs/* | sed 's/^[^ ]* //' | tail -5
