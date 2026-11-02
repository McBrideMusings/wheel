#!/usr/bin/env bash
# § Link style (wheel-h3z5.22): every link in a reply is underlined in the system link colour,
# chat.links.underline false removes the underline, chat.links.color text draws links in the body
# colour. One reply holds all six kinds of link (a Markdown link, a bare https URL, a file path, a pane
# id, an artifact name, a chat.links.patterns match). /state's `links.style` lists each link run of
# the replies with the underline and the light/dark colour the reply's inline style is told to draw
# (the same ChatLinkStyle the style reads), and the contrast of each colour against the window ground.
# A window shot (cacheDisplay) of the reply goes to $WHEEL_SHOTS (default $SCR/shots), light and dark
# by WHEEL_APPEARANCE.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS" "$SCR/alpha"
echo hello >"$SCR/alpha/notes.txt"
export CLAUDE_CONFIG_DIR="$SCR/claude"
use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
printf '[chat.links]\npatterns = [{ match = "WH-[0-9]+", action = "copy" }]\n' >>"$INST/wheel.toml"

A="$(w space new "$SCR/alpha")"
w artifact new sample --space "$A" >/dev/null
launch_app
wait_st ".active == \"$A\"" 20
P="$(st --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
w pane start "$P" agent >/dev/null
wait_st ".panes[\"$P\"].kind == \"agent\"" 15
S="$(st --arg p "$P" '.panes[$p].agent')"
say() { curl -sf -X POST -H "Authorization: Bearer $(TOKEN)" --data-binary "$1" "http://127.0.0.1:$DEBUG_PORT/agent?agent=$S" >/dev/null; }

say "A [Markdown link](https://example.com/md), a bare URL https://example.org/bare, a file $SCR/alpha/notes.txt, a pane $P, artifact sample, and a pattern WH-42."
runs() { st --arg p "$P" '.panes[$p].links.style.runs'; }
kinds() { st --arg p "$P" '[.panes[$p].links.style.runs[].kind] | sort | join(",")'; }
# The file, pane and artifact links appear once the daemon has answered the lookups.
wait_p() { wait_st ".panes[\"$P\"] | $1" "${2:-10}"; }
check "all six kinds of link are in the reply" wait_p '([.links.style.runs[].kind] | sort | join(",")) == "artifact,file,pane,pattern,web,web"' 30
echo "--- runs:"; runs | jq -c '.[] | [.kind, .text, .underline, .light, .dark]'
look() { st --arg p "$P" --arg k "$1" "[.panes[\$p].links.style.runs[] | select(.kind == \$k)][0] | $2"; }
for k in web file pane artifact pattern; do
  eq "$k link: underlined, colour link, light $(look "$k" .light), dark $(look "$k" .dark)" "$(look "$k" '"\(.underline) \(.color)"')" "true link"
done
eq "the Markdown link and the bare URL are both runs" "$(st --arg p "$P" '[.panes[$p].links.style.runs[] | select(.kind=="web") | .text] | sort | join(" | ")')" "Markdown link | https://example.org/bare"
eq "the link colour differs from the text colour in light" "$(st --arg p "$P" '[.panes[$p].links.style.contrast[] | select(.mode=="light" and (.color=="link" or .color=="text")) | .hex] | (.[0] != .[1])')" "true"
echo "--- contrast:"; st --arg p "$P" '.panes[$p].links.style.contrast[] | "\(.color) \(.mode) \(.hex) on \(.ground): \(.ratio)"'
eq "every colour reads at 4.5 to 1 or more against the ground" "$(st --arg p "$P" '[.panes[$p].links.style.contrast[] | select(.ratio < 4.5)] | length')" "0"

shot() {
  app_post "/scroll?agent=$S&from_end=0" >/dev/null; sleep 0.7
  app_post "/window/snapshot?window=$(win_of "$A")&path=$SHOTS/chat-links-$1-${WHEEL_APPEARANCE:-light}.png" >/dev/null
  echo "     shot $SHOTS/chat-links-$1-${WHEEL_APPEARANCE:-light}.png"
}
shot default

w config set --machine chat.links.underline false >/dev/null
check "underline false: no link run is underlined" wait_p '[.links.style.runs[] | select(.underline)] | length == 0' 10
shot nounderline
w config unset --machine chat.links.underline >/dev/null

w config set --machine chat.links.color text >/dev/null
check "color text: every link run is the text colour" wait_p '[.links.style.runs[] | select(.color == "text")] | length == 6' 10
eq "color text: still underlined" "$(st --arg p "$P" '[.panes[$p].links.style.runs[] | select(.underline)] | length')" "6"
shot text
w config unset --machine chat.links.color >/dev/null
check "unset: back to underlined link colour" wait_p '[.links.style.runs[] | select(.underline and .color == "link")] | length == 6' 10
finish
