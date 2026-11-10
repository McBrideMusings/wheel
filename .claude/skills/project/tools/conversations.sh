#!/usr/bin/env bash
# § An agent pane lists, resumes and starts conversations. Two fixture stores (Claude Code's under
# CLAUDE_CONFIG_DIR, Codex's under CODEX_HOME) hold conversations for the space's directory and one
# for another directory. Checks: `wheel agent conversations` lists the directory's conversations for
# a Claude pane and a Codex pane, newest first, titled by the first real prompt; the Mac app's
# state shows the same listing and a resume moves the agent into that conversation (and one a second
# pane already holds is refused); the empty conversation lists its recents. Window shots of the
# empty state and the popover go to $WHEEL_SHOTS (default $SCR/shots).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; rm -rf "$FH"; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"
FH="$(mktemp -d "${TMPDIR:-/tmp}/wheel-conv-XXXX")"
mkdir -p "$SCR/proj" "$SCR/other"
DIR="$(cd "$SCR/proj" && pwd -P)"
OTHER="$(cd "$SCR/other" && pwd -P)"
mangle() { printf '%s' "$1" | sed 's/[^A-Za-z0-9]/-/g'; }

export CLAUDE_CONFIG_DIR="$FH/claude" CODEX_HOME="$FH/codex"
CP="$CLAUDE_CONFIG_DIR/projects/$(mangle "$DIR")"
mkdir -p "$CP" "$CODEX_HOME/sessions/2026/11/10"
claude_line() { printf '{"type":"user","uuid":"u-%s","timestamp":"2026-11-10T10:00:00.000Z","sessionId":"%s","cwd":"%s","message":{"role":"user","content":"%s"}}\n' "$1" "$1" "$DIR" "$2"; }
C1=11111111-1111-4111-8111-111111111111
C2=22222222-2222-4222-8222-222222222222
C3=33333333-3333-4333-8333-333333333333
claude_line "$C1" "fix the flaky restore test" >"$CP/$C1.jsonl"; touch -t 202611010900 "$CP/$C1.jsonl"
claude_line "$C2" "add resume to the agent pane" >"$CP/$C2.jsonl"
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"- **Bold** and `code`.\\n\\nA later paragraph."}]}}\n' >>"$CP/$C2.jsonl"
touch -t 202611080900 "$CP/$C2.jsonl"
claude_line "$C3" "why does deploy-client hang" >"$CP/$C3.jsonl"
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"  It hangs\\n  on the second rename.  "}]}}\n' >>"$CP/$C3.jsonl"
touch -t 202611092000 "$CP/$C3.jsonl"
codex_file() { # id cwd prompt mtime
  printf '{"type":"session_meta","payload":{"id":"%s","cwd":"%s"}}\n{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions\\nx"}]}}\n{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"%s"}]}}\n' "$1" "$2" "$3" >"$CODEX_HOME/sessions/2026/11/10/rollout-$1.jsonl"
  touch -t "$4" "$CODEX_HOME/sessions/2026/11/10/rollout-$1.jsonl"
}
codex_file cx-1 "$DIR" "port the gallery modifiers" 202611061200
codex_file cx-2 "$DIR" "profile the transcript redraw" 202611091200
codex_file cx-3 "$OTHER" "someone else's project" 202611091300

printf '[agent.claude]\ncommand = "%s"\n[agent.codex]\ncommand = "%s"\n' "$FAKE_AGENT" "$FAKE_AGENT" >"$INST/wheel.toml"
start_daemon --state-file "$SCR/spaces.json"
prefs_reset

S="$(w space new "$DIR")"
sp() { w space ls | jq -r --arg s "$S" '[.spaces[] | select(.id==$s) | .root | .. | objects | select(.node? == "pane") | .id] | .[]'; }
while [ "$(sp | wc -l)" -lt 3 ]; do w pane split "$(sp | tail -1)" down >/dev/null; done
read -r P1 P2 P3 < <(sp | tr '\n' ' ')
w pane start "$P1" agent >/dev/null
w pane start "$P2" agent --agent codex >/dev/null
w pane start "$P3" agent >/dev/null

# 1. The CLI lists them through the same op the app uses.
listing() { w agent conversations "$1" --json 2>/dev/null | jq -r '[.[] | "\(.id)|\(.title)"] | join(",")'; }
eq "claude lists the directory's three, newest first" "$(listing "$P1")" "$C3|why does deploy-client hang,$C2|add resume to the agent pane,$C1|fix the flaky restore test"
eq "claude carries the last reply: the first paragraph, whitespace collapsed, only where there is one" "$(w agent conversations "$P1" --json 2>/dev/null | jq -r '[.[] | .last_reply // "-"] | join(",")')" "It hangs on the second rename.,- **Bold** and \`code\`.,-"
eq "a page of two is the two newest" "$(w agent conversations "$P1" --json --limit 2 2>/dev/null | jq -r '[.[].id] | join(",")')" "$C3,$C2"
CURSOR="$(w agent conversations "$P1" --json --limit 2 2>/dev/null | jq -r '.[-1].updated_at')"
eq "the page after it, by cursor, is the oldest" "$(w agent conversations "$P1" --json --before "$CURSOR" 2>/dev/null | jq -r '[.[].id] | join(",")')" "$C1"
eq "codex lists the directory's two (not the other directory's), titled past the instructions block" "$(listing "$P2")" "cx-2|profile the transcript redraw,cx-1|port the gallery modifiers"

# 2. The Mac app shows and drives it.
launch_app WHEEL_APPEARANCE=light
check "the app connects" wait_st '.connection == "connected"' 20
app_post "/focus?space=$S" >/dev/null
check "three agent panes are drawn" wait_st '[.panes[] | select(.kind == "agent")] | length == 3' 20
waitp() { wait_st "(.panes[\"$1\"] | $2)" "${3:-10}"; }
conv() { st --arg p "$1" "$2"; }
app_post "/conversations/load?pane=$P1" >/dev/null
check "the app lists the claude pane's conversations" waitp "$P1" '.conversations.state == "loaded" and (.conversations.items | length) == 3' 10
eq "the first item is the newest, with its title and an age" "$(conv "$P1" '.panes[$p].conversations.items[0] | "\(.title)|\(.age != null)"')" "why does deploy-client hang|true"
app_post "/conversations/load?pane=$P2" >/dev/null
check "the app lists the codex pane's two" waitp "$P2" '(.conversations.items | length) == 2' 10

# 3. The empty state shows its recents; the popover opens from the header button.
app_post "/window/snapshot?path=$SHOTS/empty-recents.png" >/dev/null
app_post "/pane/button?pane=$P1&button=conversations" >/dev/null
check "the popover opens" waitp "$P1" '.conversations.popover_open == true' 5
sleep 1
app_post "/window/snapshot?which=popover&path=$SHOTS/popover.png" >/dev/null || echo "     (no popover window to snapshot)"
app_post "/pane/button?pane=$P1&button=conversations-close" >/dev/null

# 4. A resume moves the agent into that conversation; the same one in another pane is refused.
app_post "/conversations/pick?pane=$P1&id=$C2" >/dev/null
check "the agent is in the resumed conversation" waitp "$P1" '.conversations.switching_to == null' 60
w agent conversations "$P1" --json 2>/dev/null | jq -e --arg c "$C2" 'map(select(.current)) | .[0].id == $c' >/dev/null && ok "the listing flags the resumed conversation current" || fail "the listing flags the resumed conversation current"
app_post "/conversations/pick?pane=$P3&id=$C2" >/dev/null
check "a second pane resuming the same conversation is refused" waitp "$P3" '.error != null and (.error | contains("already in that conversation"))' 15
app_post "/conversations/pick?pane=$P1&id=new" >/dev/null
check "New conversation empties the transcript" waitp "$P1" '.conversations.switching_to == null and .title == null' 60

# 5. While a turn runs, the app reports the rows blocked.
bg w agent send "$P3" "work slowly"
check "a running turn blocks the list" waitp "$P3" '.conversations.blocked == true' 15

finish
