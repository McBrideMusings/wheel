#!/usr/bin/env bash
# § Companion artifact (wheel-m8q): artifacts/companion draws one three.js character per active
# Claude agent from $WHEEL_DATA_DIR/agents.json, written by agents.sh.
#   1. source "wheel": against this script's daemon and the fake agent, an agent pane working a slow
#      turn is listed as working, and as waiting while a permission request is open.
#   2. The Mac app shows a pane of the artifact: with a two-transcript fixture two labelled characters,
#      with none one idle character, and after a character swap the new clip. A page that fails to
#      start (a broken .glb) shows its error as text.
# Run it once per appearance (WHEEL_APPEARANCE=light|dark); WHEEL_SHOTS=<dir> keeps the PNGs. Part 2
# needs the Mac app (WHEEL_DERIVED_DATA) and is skipped, said so, when there is none.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
APPEARANCE="${WHEEL_APPEARANCE:-light}"
mkdir -p "$SHOTS"
HAVE_APP=0
[ -d "${WHEEL_DERIVED_DATA:-$REPO/DerivedData}/Build/Products/Debug/Wheel.app" ] && HAVE_APP=1
if [ "$HAVE_APP" = 1 ]; then
  source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
  trap 'app_cleanup; cleanup' EXIT
fi

CO="$SCR/companion"          # a copy of the artifact, so config.json and the character can change
cp -R "$REPO/artifacts/companion" "$CO"
ROOTS="$SCR/roots"
mkdir -p "$ROOTS/a" "$ROOTS/b"
printf '{"cwd":"%s/alpha","type":"user"}\n' "$SCR" >"$ROOTS/a/s-alpha.jsonl"
printf '{"cwd":"%s/beta","type":"user"}\n' "$SCR" >"$ROOTS/b/s-beta.jsonl"
config() { printf '{"source":"%s","roots":["%s"],"character":"%s","max":8}\n' "$1" "$ROOTS" "${2:-robot}" >"$CO/config.json"; }

use_fake_agent
printf '[artifacts.companion]\npath = "%s"\ncommand = "WHEEL_BIN=%s /bin/bash %s/agents.sh"\nevery = 1\npane = true\n' \
  "$CO" "$WHEEL" "$CO" >>"$INST/wheel.toml"
config wheel
start_daemon --state-file "$SCR/spaces.json"

# --- 1. source "wheel" ---------------------------------------------------------------------------
mkdir -p "$SCR/proj"
S="$(w space new "$SCR/proj")"
AGENT="$(w space ls | jq -r --arg s "$S" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
w pane start "$AGENT" agent >/dev/null
AGENT="$(w status --json | jq -r --arg p "$AGENT" '[.spaces[].root | .. | objects | select(.id == $p)] | .[0].agent')"
agents() { env -u WHEEL_SPACE -u WHEEL_PANE WHEEL_URL="ws://127.0.0.1:$PORT/ws" WHEEL_TOKEN_FILE="$INST/token" \
  WHEEL_DATA_DIR="$SCR" WHEEL_BIN="$WHEEL" COMPANION_CONFIG="$CO/config.json" bash "$CO/agents.sh" && cat "$SCR/agents.json"; }
eq "wheel: a ready agent pane is not listed" "$(agents | jq -c '.agents')" "[]"
bg w agent send "$AGENT" "work slowly 10"
check "wheel: the pane's turn is running" poll 10 bash -c "[ \"\$('$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' status --json | jq -r --arg p '$AGENT' '[.spaces[].root | .. | objects | select(.id == \$p)] | .[0].state.phase')\" = busy ]"
eq "wheel: the working pane is listed as working in its project" "$(agents | jq -c '[.agents[] | [.id, .project, .state]]')" "[[\"$AGENT\",\"$SCR/proj\",\"working\"]]"
w agent cancel "$AGENT" >/dev/null 2>&1
w agent wait "$AGENT" --timeout 20 >/dev/null 2>&1
bg w agent send "$AGENT" "permission please"
check "wheel: a permission request is open" poll 20 bash -c "$(declare -f perm_id jget); WHEEL='$WHEEL' PORT=$PORT INST='$INST'; w() { \"\$WHEEL\" --url ws://127.0.0.1:\$PORT/ws --token-file \"\$INST/token\" \"\$@\"; }; [ -n \"\$(perm_id)\" ]"
eq "wheel: the pane is listed as waiting" "$(agents | jq -c '[.agents[] | [.id, .state]]')" "[[\"$AGENT\",\"waiting\"]]"
w perm answer "$(perm_id)" allow >/dev/null
w agent wait "$AGENT" --timeout 20 >/dev/null 2>&1

# --- 2. the pane ---------------------------------------------------------------------------------
if [ "$HAVE_APP" = 0 ]; then
  echo "skipped part 2: no Mac app in ${WHEEL_DERIVED_DATA:-$REPO/DerivedData}"
  finish
fi
prefs_reset
W="$(w pane split "$AGENT" right)"
w pane kind "$W" web --artifact companion --scope machine >/dev/null
config all
launch_app
check "the app connects" wait_st '.connection == "connected"' 20
bg bash -c "while :; do touch '$ROOTS'/a/s-alpha.jsonl '$ROOTS'/b/s-beta.jsonl; sleep 2; done"
shot() { sleep "${3:-12}"; w pane shot "$W" "$SHOTS/companion-$1-$APPEARANCE.png" && echo "     shot $SHOTS/companion-$1-$APPEARANCE.png"; }
w artifact refresh "$W" >/dev/null
eq "all: agents.json lists the two transcripts" "$(jq -c '[.agents[].id] | sort' "$INST/panes/$W/agents.json")" '["s-alpha","s-beta"]'
shot two
check "the shot of two agents exists" test -s "$SHOTS/companion-two-$APPEARANCE.png"

pkill -P "$BGPID" 2>/dev/null; kill "$BGPID" 2>/dev/null
rm -rf "$ROOTS/a" "$ROOTS/b"
sleep 21 # the transcripts age out of the 20 s window
w artifact refresh "$W" >/dev/null
eq "none: agents.json is empty" "$(jq -c '.agents' "$INST/panes/$W/agents.json")" "[]"
shot none 6

# A second character: the same model, with "working" mapped to Running. With one working agent the
# swap must reload the pane and show the new clip.
cp -R "$CO/characters/robot" "$CO/characters/runner"
jq '.working = "Running"' "$CO/characters/runner/character.json" >"$CO/characters/runner/c.tmp" && mv "$CO/characters/runner/c.tmp" "$CO/characters/runner/character.json"
mkdir -p "$ROOTS/a"
printf '{"cwd":"%s/alpha","type":"user"}\n' "$SCR" >"$ROOTS/a/s-alpha.jsonl"
bg bash -c "while :; do touch '$ROOTS'/a/s-alpha.jsonl; sleep 2; done"
w artifact refresh "$W" >/dev/null
shot walking
config all runner
w artifact refresh "$W" >/dev/null
shot running
check "the character swap changed the pane" bash -c "! cmp -s '$SHOTS/companion-walking-$APPEARANCE.png' '$SHOTS/companion-running-$APPEARANCE.png'"

# A broken .glb is shown as text, not a blank pane.
: >"$CO/characters/runner/RobotExpressive.glb"
shot broken 4
finish
