#!/usr/bin/env bash
# § 3f Usage cap: `wheel usage` with no login, a supervisor's cap and its defaults from settings.
# The real-login readings (5-hour and weekly percentages) need a logged-in lead and stay manual.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

STATE="$SCR/spaces.json"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
command -v bd >/dev/null || { echo "usage.sh needs bd" >&2; exit 2; }
R="$SCR/repo"
mkdir -p "$R"
git -C "$R" init -q -b main
git -C "$R" commit -q --allow-empty -m init
( cd "$R" && bd init --prefix t --quiet >/dev/null 2>&1 )
EP="$( cd "$R" && bd create "epic" --type epic --json | jget id )"
( cd "$R" && bd create "task" --parent "$EP" --json >/dev/null )

start_daemon --lead-cmd "$FAKE_AGENT" --lead-cwd /tmp --state-file "$STATE"
lead_ready() { [ "$(w status | jget lead.phase)" = ready ]; }
poll 30 lead_ready || fail "the lead is ready" "$(w status | jget lead.phase)"

expect_exit_out 3 'missing: lead is not logged in' "wheel usage with no login exits 3 and says so" w usage

# A credentials file the endpoint will not accept: still a missing reading, and the daemon logs why.
mkdir -p "$INST/lead-home"
printf '{"claudeAiOauth":{"accessToken":"x"}}\n' >"$INST/lead-home/.credentials.json"
expect_exit_out 3 'missing:' "wheel usage with a token the endpoint rejects exits 3" w usage
has_log "usage read failed" "log: usage read failed"
rm -f "$INST/lead-home/.credentials.json"

# A supervisor with a cap and no usage reading is held, and the event names the reading.
w space new "$R" >/dev/null
S="$(w supervisor start --goal "$EP" --cap 40 | jget id)"
eq "a supervisor started with --cap 40 prints cap 40" "$(w supervisor show "$S" | jget cap)" "40"
held() { w supervisor events "$S" | grep -q '"kind": "held"'; }
poll 30 held
check "a capped supervisor with no reading is held" held
check "the held event names the reading" bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' supervisor events '$S' | grep -q 'usage is unknown'"
w supervisor stop "$S" >/dev/null

# Defaults: no --cap and no --workers take the settings (90 and 3); a setting changes them.
S2="$(w supervisor start --goal "$EP")"
eq "no --cap takes the default 90" "$(printf %s "$S2" | jget cap)" "90"
eq "no --workers takes the default 3" "$(printf %s "$S2" | jget workers)" "3"
w supervisor stop "$(printf %s "$S2" | jget id)" >/dev/null
w config set supervisor.cap 40 --space s1 >/dev/null
w config set supervisor.workers 2 --space s1 >/dev/null
S3="$(w supervisor start --goal "$EP")"
eq "supervisor.cap 40 is taken" "$(printf %s "$S3" | jget cap)" "40"
eq "supervisor.workers 2 is taken" "$(printf %s "$S3" | jget workers)" "2"
SID3="$(printf %s "$S3" | jget id)"
lacks "cap with no number removes the cap" cap w supervisor cap "$SID3"
w supervisor stop "$SID3" >/dev/null

# The strip's rows from a fixture: `wheeld --usage-fixture` reads a known answer on every read.
stop_daemon
FIX="$SCR/usage.json"
printf '{"five_hour":{"utilization":42,"resets_at":"2026-10-17T20:00:00Z"},"seven_day":{"utilization":73.5},"seven_day_opus":{"utilization":30},"plan":"Max 20x","email":"a@b.c"}\n' >"$FIX"
start_daemon --lead-cmd "$FAKE_AGENT" --lead-cwd /tmp --state-file "$STATE" --usage-fixture "$FIX"
poll 30 lead_ready
expect_out '5-hour: 42%' "wheel usage prints the 5-hour reading" w usage
expect_out 'weekly opus: 30%' "wheel usage prints the per-model limit" w usage
expect_out 'plan: Max 20x' "wheel usage prints the plan" w usage
expect_out 'strip: claude weekly 73.5%' "wheel usage prints the strip's default pins" w usage
w config set usage.pins claude:opus --machine >/dev/null
expect_out 'strip: claude opus 30%' "a pin written to usage.pins reaches the strip" w usage
eq "an unpinned row leaves the strip" "$(w usage | grep -c 'strip: claude session')" "0"
expect_exit 6 "usage.pins refuses an entry with a space" w config set usage.pins "a b" --machine
expect_out 'agent: codex not shown' "codex is listed, unshown" w usage
w config set usage.agents claude,codex --machine >/dev/null
expect_out "strip: codex (can't read usage|not detected)" "a listed agent wheel cannot read says why in the strip" w usage
printf '{"fail":"HTTP 429"}\n' >"$FIX"
expect_exit_out 3 'stale: HTTP 429' "a failed read after a good one is stale and exits 3" w usage

finish
