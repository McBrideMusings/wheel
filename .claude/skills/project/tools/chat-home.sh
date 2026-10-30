#!/usr/bin/env bash
# § A chat shows the configuration directory its own session runs under (wheel-h3z5.29). Three
# agent panes in one window: a Claude pane whose saved layout names ~/.claude-work, a Claude pane
# naming ~/.claude, and a Codex pane naming ~/.codex-work, plus a Claude pane with no home of its
# own while `agent.claude.home` is unset. The status row (`chat.session.home = row`) and the
# transcript's first line (`transcript`) must show each pane's own home, and /state's home text must
# match the `agent.home` wheeld holds for the pane (`wheel space ls`). The homes are directories of
# this check's own under the real $HOME (removed on exit), so the labels start with "~".
# Window shots (cacheDisplay) of both themes go to $WHEEL_SHOTS (default $SCR/shots).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; rm -rf "$FH"; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"
FH="$HOME/.wheel-check-$$"
T="~/.wheel-check-$$"
mkdir -p "$FH/.claude-work" "$FH/.claude" "$FH/.codex-work" "$FH/.codex-other"

export CLAUDE_CONFIG_DIR="$FH/.claude"
printf '[agent.claude]\ncommand = "%s"\n[agent.codex]\ncommand = "%s"\nhome = "%s"\n' \
  "$FAKE_AGENT" "$FAKE_AGENT" "$FH/.codex-other" >"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"
prefs_reset

mkdir -p "$SCR/a"
A="$(w space new "$SCR/a")"
P1="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
sp() { w space ls | jq -r --arg s "$A" '[.spaces[] | select(.id==$s) | .root | .. | objects | select(.node? == "pane") | .id] | .[]'; }
while [ "$(sp | wc -l)" -lt 4 ]; do
  w pane split "$(sp | tail -1)" down >/dev/null
done
read -r P1 P2 P3 P4 < <(sp | tr '\n' ' ')
w pane start "$P1" agent >/dev/null
w pane start "$P2" agent >/dev/null
w pane start "$P3" agent --agent codex >/dev/null
w pane start "$P4" agent >/dev/null

# Give the panes their homes in the saved layout, as a pane switched over from a terminal has.
stop_daemon
jq --arg p1 "$P1" --arg p2 "$P2" --arg p3 "$P3" --arg h "$FH" '
  (.. | objects | select(.id? == $p1)) |= . + {home: ($h + "/.claude-work")}
  | (.. | objects | select(.id? == $p2)) |= . + {home: ($h + "/.claude")}
  | (.. | objects | select(.id? == $p3)) |= . + {home: ($h + "/.codex-work")}' "$SCR/spaces.json" >"$SCR/spaces.edited" \
  && mv "$SCR/spaces.edited" "$SCR/spaces.json"
start_daemon --state-file "$SCR/spaces.json"

launch_app WHEEL_APPEARANCE=light
check "the app connects" wait_st '.connection == "connected"' 20
app_post "/focus?space=$A" >/dev/null
check "four agent panes are drawn" wait_st '[.panes[] | select(.kind == "agent")] | length == 4' 20

daemon_home() { w space ls | jq -r --arg p "$1" '[.. | objects | select(.id? == $p) | .home?] | map(select(. != null)) | first // ""'; }
shown() { st --arg p "$1" '.panes[$p].home.text'; }
abbrev() { printf '%s' "~${1#$HOME}"; }

# 1. a Claude pane with home ~/.claude-work; 3. a pane with ~/.claude beside it.
eq "pane with ~/.claude-work shows it in the status row" "$(shown "$P1")" "$T/.claude-work"
eq "pane with ~/.claude beside it shows its own" "$(shown "$P2")" "$T/.claude"
# 4. Codex pane with its own home, over the agent.codex.home setting.
eq "the Codex pane shows its own home, not agent.codex.home" "$(shown "$P3")" "$T/.codex-work"
# 5. no home of its own and agent.claude.home unset: the agent's default.
eq "a pane with no home shows the agent's default" "$(shown "$P4")" "~/.claude"
# 6. /state's text matches wheeld's agent.home for each pane that has one.
for p in "$P1" "$P2" "$P3"; do
  eq "state home text for $p matches wheeld's pane home" "$(shown "$p")" "$(abbrev "$(daemon_home "$p")")"
done
eq "the status row is where the home shows" "$(st --arg p "$P1" '.panes[$p].home.where')" "row"

# 2. the transcript's first line.
w config set chat.session.home transcript --machine >/dev/null
check "placement becomes transcript" wait_st ".panes[\"$P1\"].home.where == \"transcript\"" 10
eq "the intro line of the work pane" "$(st --arg p "$P1" '.panes[$p].home.intro')" "Claude Code · $T/.claude-work"
eq "the intro line of the ~/.claude pane" "$(st --arg p "$P2" '.panes[$p].home.intro')" "Claude Code · $T/.claude"
eq "the intro line of the Codex pane" "$(st --arg p "$P3" '.panes[$p].home.intro')" "Codex · $T/.codex-work"

# Window shots, both themes, row placement: the two Claude panes with different homes, one above the other.
w pane close "$P3" >/dev/null
w pane close "$P4" >/dev/null
wait_st '[.panes[] | select(.kind == "agent")] | length == 2' 10
w config set chat.session.home row --machine >/dev/null
wait_st ".panes[\"$P1\"].home.where == \"row\"" 10
app_post "/window/front" >/dev/null
sleep 1
app_post "/window/snapshot?path=$SHOTS/chat-home-light.png" >/dev/null && echo "shot $SHOTS/chat-home-light.png"
kill_app
launch_app WHEEL_APPEARANCE=dark
wait_st '.connection == "connected"' 20
app_post "/focus?space=$A" >/dev/null
sleep 2
app_post "/window/snapshot?path=$SHOTS/chat-home-dark.png" >/dev/null && echo "shot $SHOTS/chat-home-dark.png"
finish
