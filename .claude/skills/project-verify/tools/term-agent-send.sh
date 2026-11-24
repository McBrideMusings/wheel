#!/usr/bin/env bash
# § 2b-term-agent-send: `wheel agent send` to a coding agent's TUI running in a terminal pane.
#
# Run it by name; all.sh does not. The first half needs no model: a fake TUI started under the
# name `codex` or `claude` records every byte it reads, so the check sees the paste and the submit
# key wheeld types. With REAL=1 the second half runs real Codex (one short prompt on the account in
# ~/.codex) in a terminal pane under Wheel's shell, so its hooks report, and sends it a multi-line
# message ending in a backslash while it is mid-turn.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# Detection reads the command line, and `node /…/codex` is how an npm-installed Codex runs. Each
# read is logged as one JSON string.
mkdir -p "$SCR/bin"
cat >"$SCR/bin/codex" <<'EOF'
const fs = require("fs");
const log = process.argv[2];
process.stdin.setRawMode(true);
process.stdout.write("\x1b[?2004hREADY-" + require("path").basename(process.argv[1]) + "> ");
process.stdin.on("data", (d) => {
  if (d.includes(3)) process.exit(0); // raw mode: Ctrl-C arrives as a byte
  fs.appendFileSync(log, JSON.stringify(d.toString("latin1")) + "\n");
});
EOF
cp "$SCR/bin/codex" "$SCR/bin/claude"

PROJ="$SCR/proj"
mkdir -p "$PROJ"
# The fake half reads conversations from a home of its own; real Codex below uses the person's.
export CODEX_HOME="$SCR/codexhome"
start_daemon
w space new "$PROJ" >/dev/null
P="$(w space ls | jget spaces.0.root.id)"
w pane start "$P" terminal >/dev/null
sleep 1

out="$(w agent send "$P" "hi" 2>&1)"; code=$?
check "a plain shell is refused" test "$code" -ne 0
check "the refusal says no coding agent runs there ($out)" grep -q "no coding agent running" <<<"$out"

# fake_tui <name> <log>: start the fake TUI in the pane under the program name <name>.
fake_tui() {
  w term send "$P" "node $SCR/bin/$1 $2" --enter
  for _ in $(seq 1 50); do grep -q "READY-$1>" <(w term read "$P" --timeout 0 2>/dev/null) && return 0; sleep 0.1; done
  return 1
}
# bytes_of <log>: what the TUI read, one read per line, once it has read the submit key.
bytes_of() { for _ in $(seq 1 30); do [ "$(wc -l <"$1")" -ge 2 ] && break; sleep 0.1; done; cat "$1"; }

: >"$SCR/codex.log"
fake_tui codex "$SCR/codex.log" || fail "the fake codex starts"
msg=$'line one\nline two ends in a backslash \\'
err="$(w agent send "$P" "$msg" 2>&1 >/dev/null)"; code=$?
eq "agent send to codex in a terminal exits 0" "$code" "0"
check "it says the message was submitted ($err)" grep -q "submitted" <<<"$err"
got="$(bytes_of "$SCR/codex.log")"
eq "codex gets one bracketed paste, newline kept, a space after the backslash, then Tab" "$got" \
  "$(printf '%s\n%s' '"\u001b[200~line one\nline two ends in a backslash \\ \u001b[201~"' '"\t"')"

: >"$SCR/codex.log"
w agent send "$P" "/status" >/dev/null 2>&1
eq "a slash command to codex is submitted with Enter" "$(bytes_of "$SCR/codex.log" | tail -1)" '"\r"'

out="$(w agent wait "$P" --timeout 1 2>&1)"; code=$?
check "agent wait on an agent with no hooks fails ($code)" test "$code" -ne 0
check "it names what is missing ($out)" grep -q "reported no status" <<<"$out"
out="$(w agent read "$P" 2>&1)"
check "agent read says there is no transcript and how to see the screen ($out)" grep -q "no transcript.*wheel term read $P" <<<"$out"
eq "agent context names the agent in its pane" "$(w agent context "$P" 2>&1)" "codex in $P unknown"
out="$(w agent cancel "$P" 2>&1)"
check "other agent verbs say what reaches it ($out)" grep -q "runs Codex in a terminal" <<<"$out"

# A rollout for the pane's directory, written after the fake started, is the conversation the
# sampler adopts; from then on the pane takes the hooks' reports, driven here by hand.
mkdir -p "$CODEX_HOME/sessions/2099/01/01"
printf '{"payload":{"id":"01a20000-0000-7000-8000-000000000001","cwd":"%s"}}\n' "$PROJ" \
  >"$CODEX_HOME/sessions/2099/01/01/rollout-fake.jsonl"
for _ in $(seq 1 40); do [ "$(w space ls | jget spaces.0.root.agent.name)" = codex ] && break; sleep 0.25; done
eq "the sampler adopts the fake codex" "$(w space ls | jget spaces.0.root.agent.name)" "codex"
w pane report working --pane "$P"
: >"$SCR/codex.log"
err="$(w agent send "$P" "while busy" 2>&1 >/dev/null)"
check "a send while the hooks say working is queued ($err)" grep -q "queued behind its running turn" <<<"$err"
eq "and still submitted with Tab" "$(bytes_of "$SCR/codex.log" | tail -1)" '"\t"'
w agent wait "$P" --timeout 1 >/dev/null 2>&1; code=$?
eq "agent wait times out while it works" "$code" "3"
w pane report idle --pane "$P"
out="$(w agent wait "$P" --timeout 5 2>&1)"; code=$?
eq "agent wait returns once the hooks say idle ($out)" "$code" "0"
w agent send "$P" "while idle" >/dev/null 2>&1
eq "a send marks the pane working at once, before the hook" "$(w space ls | jget spaces.0.root.status.kind)" "working"
w pane report needs_you --pane "$P"
: >"$SCR/codex.log"
out="$(w agent send "$P" "y" 2>&1)"; code=$?
check "a send while it waits on a permission is refused ($out)" grep -q "permission prompt" <<<"$out"
sleep 0.3
eq "and types nothing" "$(cat "$SCR/codex.log")" ""

w term send "$P" --key ctrl-c >/dev/null
sleep 0.5
w term send "$P" "clear" --enter
sleep 0.5
: >"$SCR/claude.log"
fake_tui claude "$SCR/claude.log" || fail "the fake claude starts"
w agent send "$P" "hello" >/dev/null 2>&1
eq "claude gets Enter" "$(bytes_of "$SCR/claude.log" | tail -1)" '"\r"'
w term send "$P" --key ctrl-c >/dev/null

[ "${REAL:-}" = 1 ] || finish
command -v codex >/dev/null || { echo "needs codex on PATH for REAL=1" >&2; finish; }

# Real Codex under Wheel's login shell, so its wrapper's hooks report to this daemon.
stop_daemon
unset CODEX_HOME
DAEMON_SHELL=/bin/zsh start_daemon
w space new "$PROJ" >/dev/null
P="$(w space ls | jget spaces.0.root.id)"
w pane start "$P" terminal >/dev/null
sleep 2
w term send "$P" "codex -s read-only -a never" --enter
w term read "$P" --until "Ask Codex" --timeout 30 >/dev/null 2>&1 || { fail "Codex starts" "$(w term read "$P" --timeout 0 2>&1 | tail -5)"; finish; }
sleep 3   # the composer draws before Codex reads its input
w agent send "$P" 'Run the shell command `sleep 30`, then reply with the single word first.' 2>&1
# Mid-turn by the screen: in a throwaway daemon the sampler may not adopt the conversation, so
# the hooks' reports can be dropped and the send says "submitted"; Tab delivers either way.
turning=""
for _ in $(seq 1 60); do w term read "$P" --timeout 0 2>/dev/null | grep -aq "to interrupt" && { turning=1; break; }; sleep 0.5; done
[ -n "$turning" ] && ok "Codex is mid-turn (its screen offers esc to interrupt)" || fail "Codex starts its turn" "$(w term read "$P" --timeout 0 2>&1 | tr -c '[:print:]\n' ' ' | tail -c 1500)"
sleep 3
w term read "$P" --timeout 0 2>/dev/null | grep -aq "to interrupt" && ok "still mid-turn when the message is sent" || fail "still mid-turn when the message is sent"
w agent send "$P" $'Mid-turn, two lines.\nReply with the word ZEBRA spelled backwards, in capitals \\' >/dev/null 2>&1
w agent wait "$P" --timeout 120 >/dev/null 2>&1
w term read "$P" --until "ARBEZ" --timeout 120 >/dev/null 2>&1 && ok "Codex answers the queued message" \
  || fail "Codex answers the queued message" "$(w term read "$P" --timeout 0 2>&1 | tail -10)"
[ -n "${KEEP_LOGS:-}" ] && cp -R "$LOGS" "$KEEP_LOGS"
finish
