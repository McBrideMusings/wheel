#!/usr/bin/env bash
# § An artifact entry's description reaches the CLI's text form and the agent's brief (wheel-k5yn):
# `wheel artifact ls` prints it under the name line (an entry without one prints no extra line), and
# a chat agent started with either of the person's Claude configuration directories (~/.claude and
# ~/.claude-work, as agent.claude.home) gets the same text in the brief's artifacts section. Drives
# this script's own daemon with 16 spaces. The pane picker is palette-description.sh.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
trap cleanup EXIT
mkspaces() { local i; for i in $(seq 1 "$1"); do mkdir -p "$SCR/proj$i"; w space new "$SCR/proj$i"; done; }

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
SPACES=(); while read -r id; do SPACES+=("$id"); done < <(mkspaces 16)
S1="${SPACES[12]}"

w artifact new described --pane --space "$S1" --description "Shows the swarm's builds." >/dev/null
w artifact new plain --pane --space "$S1" >/dev/null

TXT="$(w artifact ls --space "$S1")"
echo "$TXT" | sed 's/^/     ls: /'
eq "ls text prints the description on the line under the name" "$(printf '%s\n' "$TXT" | grep -A1 '^project described ' | tail -1)" "  Shows the swarm's builds."
eq "ls text gives the entry with none no description line" "$(printf '%s\n' "$TXT" | grep -A1 '^project plain ' | tail -1 | grep -c "Shows the swarm")" 0
eq "ls text still lists the entry without one" "$(printf '%s\n' "$TXT" | grep -c '^project plain ')" 1

for home in "$HOME/.claude" "$HOME/.claude-work"; do
  [ -d "$home" ] || { fail "$home exists" "no such directory"; continue; }
  w config set --machine agent.claude.home "$home" >/dev/null
  w space focus "$S1" >/dev/null 2>&1 || true
  P="$(w space ls | jq -r --arg s "$S1" '[.spaces[] | select(.id == $s)][0] | [.. | objects | select(.id? and .kind?) | .id] | first')"
  w pane start "$P" agent --message "show setup" >/dev/null
  w agent wait "$P" --timeout 10 >/dev/null
  SEEN="$(w agent read "$P")"
  echo "$SEEN" | grep -E "config_dir|artifacts" | sed 's/^/     /'
  expect_out "config_dir=$home" "the agent runs with $home" echo "$SEEN"
  expect_out "- described \(project\): Shows the swarm's builds\." "the brief under $home carries the description" echo "$SEEN"
  w pane kind "$P" empty >/dev/null 2>&1 || true
done

finish
