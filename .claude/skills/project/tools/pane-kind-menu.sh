#!/usr/bin/env bash
# § 4 A pane header's Change Type menu (wheel-36et): Shell, each found agent, Browser, Artifact…
# and Empty, run through `POST /pane/menu`, the call the menu's buttons make, and read back from
# `/state` and `wheel space ls`. The ⋯ button and the right-click draw the same `menuContent`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
mkdir -p "$SCR/alpha"
S="$(w space new "$SCR/alpha")"
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 1)' 20
P="$(st --arg s "$S" '.spaces[] | select(.id==$s) | .focus')"
kind_of() { st --arg p "$P" '[.spaces[].root | .. | objects | select(.node=="pane" and .id==$p)][0].kind'; }
pick() { app_post "/pane/menu?pane=$P&item=$1" >/dev/null; }
status() { curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $(TOKEN)" "http://127.0.0.1:$DEBUG_PORT/pane/menu?pane=$P&item=$1"; }
wait_kind() { wait_st --arg p "$P" "[.spaces[].root | .. | objects | select(.node==\"pane\" and .id==\$p)][0].kind == \"$1\"" "${2:-10}"; }

eq "the menu lists Change Type items in order" "$(st --arg p "$P" '[.panes[$p].menu[] | select(.id | startswith("kind:")) | .id | select((startswith("kind:agent:") | not) or . == "kind:agent:claude")] | join(",")')" \
  "kind:shell,kind:agent:claude,kind:browser,kind:artifact,kind:empty"
eq "Empty is off on an empty pane" "$(st --arg p "$P" '.panes[$p].menu[] | select(.id=="kind:empty") | .enabled')" "false"

pick kind:shell
wait_kind terminal
eq "Shell makes the pane a terminal" "$(kind_of)" "terminal"
eq "Shell is off on a shell" "$(st --arg p "$P" '.panes[$p].menu[] | select(.id=="kind:shell") | .enabled')" "false"

pick kind:agent:claude
wait_st '.confirmation != null' 10
eq "Claude Code on a shell asks first" "$(st .confirmation.title)" "Kill this pane's shell?"
app_post "/confirm?answer=yes" >/dev/null
wait_kind agent 15
eq "Claude Code makes the pane an agent" "$(kind_of)" "agent"
eq "the shell's terminal is released" "$(w status --json | jq '[.terminals[]? | select(.alive)] | length')" "0"

pick kind:browser
wait_st '.confirmation != null' 10
eq "Browser on an agent asks first" "$(st .confirmation.title)" "End this pane's agent session?"
app_post "/confirm?answer=yes" >/dev/null
wait_kind browser
eq "Browser makes the pane a browser" "$(kind_of)" "browser"

pick kind:empty
wait_kind empty
eq "Empty makes the pane empty (a browser asks nothing)" "$(kind_of)" "empty"

eq "a disabled item is refused with 409" "$(status kind:empty)" "409"
finish
