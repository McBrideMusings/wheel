#!/usr/bin/env bash
# § 3f Usage: `wheel usage` and the strip's rows from a fixture: every window a reading carries and
# none it leaves out, the usage.meters and usage.labels settings, a Codex account whose only window
# is weekly, a missing login and a failed read. The real-login readings stay manual.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

STATE="$SCR/spaces.json"

# The strip's rows from a fixture: `wheeld --usage-fixture` reads a known answer on every read.
FIX="$SCR/usage.json"
printf '{"five_hour":{"utilization":42,"resets_at":"2026-10-17T20:00:00Z"},"seven_day":{"utilization":73.5},"limits":[{"kind":"weekly_scoped","group":"weekly","percent":30,"scope":{"model":{"display_name":"Fable"}}}],"billing":"Max 20x","email":"a@b.c"}\n' >"$FIX"
start_daemon --state-file "$STATE" --usage-fixture "$FIX"
expect_out '^5h: 42% \(resets 2026-10-17T20:00:00Z, 18000 s\)' "wheel usage prints the 5-hour window with its reset and length" w usage
expect_out '^weekly: 73.5% \(resets -, 604800 s\)' "wheel usage prints the weekly window" w usage
expect_out '^weekly-fable: 30%' "wheel usage prints the model-scoped limit as its own window" w usage
expect_out 'billing: Max 20x' "wheel usage prints the billing" w usage
expect_out 'source: oauth/usage via Claude Code logins' "a Claude account names the Claude source" w usage
expect_out 'strip: claude Claude 5h 42%' "the default usage.meters put the 5-hour window on the strip" w usage
expect_out 'strip: claude Claude weekly 73.5%' "...and the weekly window" w usage
eq "a window not in usage.meters is off" "$(w usage | grep -c 'strip: claude Claude weekly-fable')" "0"
w config set usage.meters claude:weekly-fable:full --machine >/dev/null
expect_out 'strip: claude Claude weekly-fable 30% full' "a meter written to usage.meters reaches the strip with its level" w usage
eq "the unlisted windows leave the strip" "$(w usage | grep -c 'strip: claude Claude 5h')" "0"
expect_exit 6 "usage.meters refuses an entry with no level" w config set usage.meters claude:5h --machine
expect_exit 6 "usage.meters refuses a level that is not meter or full" w config set usage.meters claude:5h:off --machine
expect_exit 6 "usage.labels refuses a label it does not know" w config set usage.labels plan --machine
expect_exit 0 "usage.labels takes billing, folder and email" w config set usage.labels billing,folder,email --machine
expect_out '^billing,folder,email' "...and lists them" w config get usage.labels
w config set usage.agents claude --machine >/dev/null
expect_out 'agent: codex not shown' "an agent usage.agents leaves out is listed, unshown" w usage
w config set usage.agents claude,codex --machine >/dev/null
expect_out "strip: codex (can't read usage|not detected)" "a listed agent wheel cannot read says why in the strip" w usage

# An idle Claude account: five_hour is null, so there is no 5-hour window, not a 0% one.
printf '{"five_hour":null,"seven_day":{"utilization":19,"resets_at":"2026-11-11T07:00:01+00:00"},"limits":[{"kind":"weekly_all","group":"weekly","percent":19,"scope":null},{"kind":"session","group":"session","percent":3,"scope":null}]}\n' >"$FIX"
w config set usage.meters claude:5h:meter,claude:weekly:meter --machine >/dev/null
eq "an idle account has no 5h window" "$(w usage | grep -c '^5h:')" "0"
expect_out '^weekly: 19%' "...and keeps its weekly one" w usage
eq "the claude:5h meter draws nothing for it" "$(w usage | grep -c 'strip: claude Claude 5h')" "0"

printf '{"fail":"HTTP 429"}\n' >"$FIX"
expect_exit_out 3 'stale: HTTP 429' "a failed read after a good one is stale and exits 3" w usage
printf '{"not_logged_in":"signed out"}\n' >"$FIX"
expect_exit_out 3 'missing: signed out' "a login that is gone is missing and exits 3" w usage

# Several accounts, one entry each, named by directory; a signed-out one says so and the other still
# reads. A Codex account carries a Codex-shaped answer: a Pro Lite login's only window is the weekly
# one (primary_window of 604800 s, no secondary), named by its length.
printf '{"accounts":[{"name":".claude","five_hour":{"utilization":3},"seven_day":{"utilization":86}},{"name":".claude-work","not_logged_in":"signed out: open Claude Code with /h/.claude-work to sign in again"},{"name":".codex-work","agent":"codex","plan_type":"prolite","rate_limit":{"primary_window":{"used_percent":92,"limit_window_seconds":604800,"reset_at":1794629355},"secondary_window":null}}]}\n' >"$FIX"
w config set usage.meters claude:5h:meter,claude:weekly:meter,codex:5h:meter,codex:weekly:full --machine >/dev/null
expect_out 'strip: claude .claude weekly 86%' "an account is named by its directory" w usage
expect_out 'strip: claude .claude-work signed out: open Claude Code' "a signed-out account says so in the strip" w usage
expect_out '^account: .codex-work \(codex\)' "wheel usage lists the Codex account" w usage
expect_out '^weekly: 92% \(resets 2026-11-14T04:09:15Z, 604800 s\)' "...with one weekly window from its 604800 s primary window" w usage
eq "...and no 5h window" "$(w usage | sed -n '/account: .codex-work/,$p' | grep -c '^5h:')" "0"
expect_out 'billing: Prolite' "...and its plan as billing" w usage
expect_out 'source: wham/usage via Codex logins' "a Codex account names the Codex source" w usage
expect_out 'strip: codex .codex-work weekly 92% full' "the codex:weekly:full meter draws it at the full level" w usage
eq "the codex:5h meter draws nothing for a login with no 5h window" "$(w usage | grep -c 'strip: codex .codex-work 5h')" "0"
has_log "usage read failed account=.claude-work" "log: the signed-out account's read names it"
has_log "usage read account=.codex-work source=wham/usage via Codex logins windows=weekly=92%/604800s billing=Prolite" "log: a read names its source, windows and billing"
stop_daemon

# An agent pane's entry in `wheel status --json` carries its login (wheel-32cu.1): the home the
# agent takes its login from, and the billing and email of the account read from that folder,
# whatever usage.labels says. A home no reading names gives the home alone.
WORK="$SCR/h/.claude-work"
mkdir -p "$WORK"
printf '{"accounts":[{"name":".claude-work","dirs":["%s"],"five_hour":{"utilization":3},"seven_day":{"utilization":86},"billing":"Max 20x","email":"pat@work.example"}]}\n' "$WORK" >"$FIX"
use_fake_agent
printf 'home = "%s/"\n' "$WORK" >>"$INST/wheel.toml"
start_daemon --state-file "$STATE" --usage-fixture "$FIX"
w config set usage.labels folder --machine >/dev/null
w space new /tmp >/dev/null
LOGIN_PANE="$(w space ls | jget spaces.0.root.id)"
w pane start "$LOGIN_PANE" agent >/dev/null
w usage >/dev/null
eq "status --json: login.home is the agent.claude.home setting, trailing / dropped" "$(w status --json | jget spaces.0.root.login.home)" "$WORK"
eq "status --json: login.billing comes from the reading of that home" "$(w status --json | jget spaces.0.root.login.billing)" "Max 20x"
eq "status --json: login.email too, with usage.labels naming only folder" "$(w status --json | jget spaces.0.root.login.email)" "pat@work.example"
w space new /tmp >/dev/null
eq "the second space's pane runs no agent" "$(w status --json | jget spaces.1.root.kind)" "empty"
eq "...and has no login" "$(w status --json | python3 -c 'import json,sys; print("login" in json.load(sys.stdin)["spaces"][1]["root"])')" "False"
printf '{"accounts":[{"name":".claude","dirs":["%s"],"five_hour":{"utilization":3},"seven_day":{"utilization":86},"billing":"Pro","email":"a@b.c"}]}\n' "$SCR/h/.claude" >"$FIX"
w usage >/dev/null
eq "a reading that no longer names the home leaves billing out" "$(w status --json | jget spaces.0.root.login.billing)" ""
eq "...and keeps the home" "$(w status --json | jget spaces.0.root.login.home)" "$WORK"
check "the saved layout carries no login" file_lacks '"login"' "$STATE"

finish
