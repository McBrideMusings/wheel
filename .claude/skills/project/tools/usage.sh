#!/usr/bin/env bash
# § 3f Usage: `wheel usage` and the strip's rows from a fixture, a missing login and a failed read.
# The real-login readings (5-hour and weekly percentages) need a logged-in account and stay manual.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

STATE="$SCR/spaces.json"

# The strip's rows from a fixture: `wheeld --usage-fixture` reads a known answer on every read.
FIX="$SCR/usage.json"
printf '{"five_hour":{"utilization":42,"resets_at":"2026-10-17T20:00:00Z"},"seven_day":{"utilization":73.5},"limits":[{"kind":"weekly_scoped","group":"weekly","percent":30,"scope":{"model":{"display_name":"Fable"}}}],"plan":"Max 20x","email":"a@b.c"}\n' >"$FIX"
start_daemon --state-file "$STATE" --usage-fixture "$FIX"
expect_out '5-hour: 42%' "wheel usage prints the 5-hour reading" w usage
expect_out 'weekly fable: 30%' "wheel usage prints the per-model limit from the limits list" w usage
expect_out 'plan: Max 20x' "wheel usage prints the plan" w usage
expect_out 'strip: claude Claude weekly 73.5%' "wheel usage prints the strip's default pins" w usage
w config set usage.pins claude:fable --machine >/dev/null
expect_out 'strip: claude Claude fable 30%' "a pin written to usage.pins reaches the strip" w usage
eq "an unpinned row leaves the strip" "$(w usage | grep -c 'strip: claude Claude session')" "0"
expect_exit 6 "usage.pins refuses an entry with a space" w config set usage.pins "a b" --machine
expect_out 'agent: codex not shown' "codex is listed, unshown" w usage
w config set usage.agents claude,codex --machine >/dev/null
expect_out "strip: codex (can't read usage|not detected)" "a listed agent wheel cannot read says why in the strip" w usage
printf '{"fail":"HTTP 429"}\n' >"$FIX"
expect_exit_out 3 'stale: HTTP 429' "a failed read after a good one is stale and exits 3" w usage
printf '{"not_logged_in":"signed out"}\n' >"$FIX"
expect_exit_out 3 'missing: signed out' "a login that is gone is missing and exits 3" w usage

# Several accounts, one entry each, named by directory; a signed-out one says so and the other still reads.
printf '{"accounts":[{"name":".claude","five_hour":{"utilization":3},"seven_day":{"utilization":86}},{"name":".claude-work","not_logged_in":"signed out: open Claude Code with /h/.claude-work to sign in again"}]}\n' >"$FIX"
w config set usage.pins claude:session,claude:weekly --machine >/dev/null
expect_out 'strip: claude .claude weekly 86%' "an account is named by its directory" w usage
expect_out 'strip: claude .claude-work signed out: open Claude Code' "a signed-out account says so in the strip" w usage
expect_out '^weekly: 86%' "wheel usage reads the logged-in account" w usage
has_log "usage read failed account=.claude-work" "log: the signed-out account's read names it"

finish
