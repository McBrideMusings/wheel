#!/usr/bin/env bash
# § 3f Usage cap: `wheel usage` with no login, a supervisor's cap and its defaults from settings.
# The real-login readings (5-hour and weekly percentages) need a logged-in lead and stay manual.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

STATE="$SCR/spaces.json"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
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
eq "cap with no number removes the cap" "$(w supervisor cap "$SID3" | jget cap)" ""
w supervisor stop "$SID3" >/dev/null

finish
