#!/bin/bash
# The terminal-to-chat switch reads the status the agent's hooks reported, never its transcript.
# A fake claude runs in a terminal pane; `wheel pane report` plays the hooks. Needs
# $CARGO_TARGET_DIR (wheeld, wheel, wheel-fake-agent built).
set -u
T=${CARGO_TARGET_DIR:-$(cd "$(dirname "$0")/../../../.." && pwd)/target}/debug
SCR=$(mktemp -d)
PORT=$((7500 + $(printf %s "${SWARM_WORKER:-x}" | cksum | cut -d" " -f1) % 400))
mkdir -p "$SCR/inst" "$SCR/logs" "$SCR/bin" "$SCR/cfg" "$SCR/space"
ln -s "$T/wheel-fake-agent" "$SCR/bin/claude"
printf '[terminal]\nshell = "%s/shell.sh"\n' "$SCR" > "$SCR/space/wheel.toml"
printf '[agent.claude]\ncommand = "%s"\n' "$T/wheel-fake-agent" > "$SCR/inst/wheel.toml"
cat > "$SCR/shell.sh" <<SH
#!/bin/sh
export PATH=$SCR/bin:\$PATH CLAUDE_CONFIG_DIR=$SCR/cfg
exec /bin/sh -i
SH
chmod +x "$SCR/shell.sh"
cd "$SCR" || exit 2
"$T/wheeld" --listen 127.0.0.1:$PORT --support-dir inst --machine-file inst/wheel.toml --log-dir logs >daemon.out 2>&1 &
D=$!
trap 'kill $D 2>/dev/null; rm -rf "$SCR"' EXIT
export WHEEL_URL=ws://127.0.0.1:$PORT/ws WHEEL_TOKEN_FILE=$SCR/inst/token
unset WHEEL_PANE WHEEL_SPACE
W=$T/wheel
for _ in $(seq 40); do [ -f inst/token ] && "$W" space ls >/dev/null 2>&1 && break; sleep 0.25; done
"$W" space new "$SCR/space" >/dev/null
P=$("$W" space ls | python3 -I -c 'import json,sys;print(json.load(sys.stdin)["spaces"][0]["focus"])')
"$W" pane kind "$P" terminal >/dev/null
FAIL=0
# expect <label> <want: ok|refused> <pane view output and exit code>
try() {
  local label=$1 want=$2 out code
  out=$("$W" pane view "$P" agent 2>&1); code=$?
  if [ "$want" = ok ] && [ $code = 0 ]; then echo "PASS $label: $out"
  elif [ "$want" = refused ] && [ $code != 0 ]; then echo "PASS $label: $out"
  else echo "FAIL $label (want $want): exit $code: $out"; FAIL=1; fi
}
# Each case starts a fresh fake claude in the pane, waits for the 2 s sampler to see it.
start() {
  "$W" term send "$P" "claude --session-id 11111111-2222-3333-4444-5555555555$1" --enter
  for _ in $(seq 30); do
    [ "$("$W" pane view "$P" | awk '{print $2}')" != "-" ] && break; sleep 0.5
  done
  "$W" term send "$P" "hello" --enter; sleep 0.5
}
report() { "$W" pane report "$1" --pane "$P"; sleep 0.3; }
# Back to the terminal once the chat agent has started, then out of the fake claude.
back() {
  for _ in $(seq 40); do "$W" pane view "$P" terminal >/dev/null 2>&1 && break; sleep 0.5; done
  "$W" term send "$P" "/exit" --enter; sleep 3
}

start 01; try "case 6 no status reported" ok; back
start 02; report working;   try "case 2 working" refused
report needs_you;           try "case 3 needs_you" refused
report working; "$W" term send "$P" "" --key esc; sleep 0.8; try "case 5 esc while working" ok; back
start 03; report working; report idle; try "case 4 stop -> idle" ok; back
grep -h 'turn check before the agent view\|terminal input changed' logs/* | sed 's/^[^ ]* //' | cut -c1-200
[ $FAIL = 0 ] && echo "all switch-by-status checks passed" || { echo "FAILED"; exit 1; }
