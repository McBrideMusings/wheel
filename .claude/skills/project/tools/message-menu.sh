#!/usr/bin/env bash
# § Right-click on a message (wheel-h3z5.24): the person's message and the agent's reply open the
# same menu: Copy message, Copy message as Markdown, a separator, Attach message as context. Drives
# the real Mac app through its debug server against this script's own daemon: `POST /agent/message`
# builds the menu a right-click opens and runs the item the way a click does; `/state` panes.<p>
# `.message_menu` lists the items of the last menu built. The pasteboard is read with pbpaste (the
# check overwrites it). Window shots (cacheDisplay) go to $WHEEL_SHOTS (default $SCR/shots).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset

mkdir -p "$SCR/a"
A="$(w space new "$SCR/a")"
P="$(w space ls | jq -r --arg s "$A" '.spaces[] | select(.id==$s) | .focus')"
w pane start "$P" agent >/dev/null

PROMPT='Use **bold** and `code` here'
w agent send "$P" "$PROMPT" >/dev/null

launch_app
check "the app connects" wait_st '.connection == "connected"' 20
app_post "/focus?space=$A" >/dev/null
check "the reply arrives" wait_st ".panes[\"$P\"].transcript_messages | length == 1" 20
S="$(st --arg p "$P" '.panes[$p].agent')"
echo "--- items:"; st --arg p "$P" '.panes[$p].transcript_items[]'
REPLY_ROW="$(st --arg p "$P" '.panes[$p].transcript_messages[0].row')"
USER_ROW="$(st --arg p "$P" '.panes[$p].transcript_items[]' | grep -E '^(row|user)' | head -1 | awk '{print $2}')"
echo "user row=$USER_ROW reply row=$REPLY_ROW"

msg() { app_post "/agent/message?agent=$S&row=$1&action=$2" >/dev/null; }
menu() { st --arg p "$P" '.panes[$p].message_menu | tojson'; }
ITEMS='["Copy message","Copy message as Markdown","-","Attach message as context"]'

msg "$REPLY_ROW" copy
eq "1. a reply's menu: Copy, Copy as Markdown, a separator, Attach, nothing else" "$(menu)" "$ITEMS"
PLAIN="$(pbpaste)"
echo "plain: $PLAIN"
check "3. Copy message puts the text without Markdown markers on the pasteboard" \
  bash -c '[[ "$1" == *"bold and code here"* && "$1" != *"**"* && "$1" != *"\`"* ]]' _ "$PLAIN"
msg "$REPLY_ROW" markdown
MD="$(pbpaste)"
echo "markdown: $MD"
check "4. Copy message as Markdown puts the source, markers kept, on the pasteboard" \
  bash -c '[[ "$1" == *"**bold**"* && "$1" == *"\`code\`"* ]]' _ "$MD"

printf %s stale | pbcopy
msg "$USER_ROW" markdown
eq "2/4. the person's message copied as Markdown is the text as typed" "$(pbpaste)" "$PROMPT"
printf %s stale | pbcopy
msg "$USER_ROW" copy
eq "2/3. the person's message copied as plain text is the text as typed" "$(pbpaste)" "$PROMPT"
eq "2. the person's message opens the same menu" "$(menu)" "$ITEMS"

app_post "/agent/draft?agent=$S" >/dev/null
msg "$USER_ROW" attach
check "5. Attach puts the message in the composer as a quoted block" \
  wait_st ".panes[\"$P\"].composer | startswith(\"> $PROMPT\")" 5
echo "composer: $(st --arg p "$P" '.panes[$p].composer')"
check "6. no item names Pin or Fork" \
  bash -c '[[ "$1" != *Pin* && "$1" != *Fork* ]]' _ "$(menu)"
if app_post "/agent/message?agent=$S&row=-999&action=copy" >/dev/null 2>&1; then
  fail "a row that is not a message is refused" "accepted"
else ok "a row that is not a message is refused"; fi

app_post "/window/front" >/dev/null
sleep 1
app_post "/window/snapshot?path=$SHOTS/message-menu-light.png" >/dev/null && echo "shot $SHOTS/message-menu-light.png"
kill_app
launch_app WHEEL_APPEARANCE=dark
wait_st '.connection == "connected"' 20
app_post "/focus?space=$A" >/dev/null
sleep 2
app_post "/window/snapshot?path=$SHOTS/message-menu-dark.png" >/dev/null && echo "shot $SHOTS/message-menu-dark.png"
finish
