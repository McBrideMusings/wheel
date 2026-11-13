# Shared by the scripts that drive a WheelPhoneUITests test against their own wheeld: a pool
# simulator, the test run, the step/ack handshake and phone.log. Source after lib.sh, then call
# ios_ui_start <Class/testMethod> [VAR=value ...] (each pair becomes a TEST_RUNNER_ variable).
SIM=~/.claude/tools/sim
BUNDLE=com.piercemakes.wheel.phone
UDID=""
TPID=""
PLOG=""
ios_cleanup() {
  [ -n "$DPID" ] && kill -CONT "$DPID" 2>/dev/null
  [ -n "$TPID" ] && kill "$TPID" 2>/dev/null
  [ -n "$UDID" ] && "$SIM" release >/dev/null 2>&1
  cleanup
}
trap ios_cleanup EXIT

DD="${WHEEL_IOS_DERIVED_DATA:-}"
[ -d "$DD/Build/Products" ] || { echo "set WHEEL_IOS_DERIVED_DATA to a build-for-testing of WheelPhone" >&2; exit 2; }
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
SYNC="$SCR/sync"
mkdir -p "$SHOTS" "$SYNC"

# ios_ui_start <test> [VAR=value ...]: pair a fresh install on a pool simulator and start the test.
ios_ui_start() {
  local test="$1" kv; shift
  UDID="$("$SIM" acquire)"
  xcrun simctl uninstall "$UDID" "$BUNDLE" >/dev/null 2>&1
  local code xrun; code="$(w remote pair --address 127.0.0.1)"
  xrun="$(ls "$DD"/Build/Products/*.xctestrun | head -1)"
  local envs=(TEST_RUNNER_WHEEL_PAIR_JSON="$code" TEST_RUNNER_WHEEL_SYNC_DIR="$SYNC" TEST_RUNNER_WHEEL_SHOTS="$SHOTS")
  for kv in "$@"; do envs+=("TEST_RUNNER_$kv"); done
  env "${envs[@]}" xcodebuild test-without-building -xctestrun "$xrun" -destination "id=$UDID" \
    -only-testing:"WheelPhoneUITests/$test" >"$SCR/xcodebuild.log" 2>&1 &
  TPID=$!
}

# plog <text>: phone.log holds the text.
plog() {
  if [ -z "$PLOG" ]; then
    local data; data="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data 2>/dev/null)" || return 1
    PLOG="$data/Documents/phone.log"
  fi
  grep -q -- "$1" "$PLOG" 2>/dev/null
}

# step <name>: wait for the test to reach it; ack <name> lets the test go on.
step() {
  local i
  for i in $(seq 1 1200); do
    [ -f "$SYNC/step-$1" ] && return 0
    { kill -0 "$TPID" 2>/dev/null && ! grep -qE "Test Suite 'Selected tests' (passed|failed)" "$SCR/xcodebuild.log"; } || { fail "the UI test reached $1" "$(grep -E 'error:|XCTAssert' "$SCR/xcodebuild.log" | head -5)"; return 1; }
    sleep 0.25
  done
  fail "the UI test reached $1 within 5 minutes"
  return 1
}
ack() { touch "$SYNC/ack-$1"; }

# ios_ui_finish <shot prefix>: wait for the suite's end, check it passed, list the shots, finish.
ios_ui_finish() {
  local test_exit
  for _ in $(seq 1 120); do grep -qE "Test Suite 'Selected tests' (passed|failed)" "$SCR/xcodebuild.log" && break; sleep 0.5; done
  if grep -q "Test Suite 'Selected tests' passed" "$SCR/xcodebuild.log"; then test_exit=0; else test_exit=1; fi
  kill "$TPID" 2>/dev/null
  TPID=""
  eq "the UI test passed" "$test_exit" 0
  [ "$test_exit" = 0 ] || grep -E "error:|XCTAssert" "$SCR/xcodebuild.log" | head -10
  for f in "$SHOTS"/"$1"-*.png; do echo "shot $f"; done
  plog . && cp "$PLOG" "$SHOTS/$1-phone.log" && echo "phone.log $SHOTS/$1-phone.log"
  finish
}
