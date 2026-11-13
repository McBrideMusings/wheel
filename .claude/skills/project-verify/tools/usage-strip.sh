#!/usr/bin/env bash
# § 3f The usage strip and window (wheel-9jv0.1): the strip draws each login's metered windows from
# its own reading (a Pro Lite Codex login has one weekly window and no 5-hour one), the Usage window
# offers an Off/Meter/Full picker per window, `full` adds the pace tick and the reset clock, and the
# labels come from usage.labels (the folder only when an agent has two or more logins). Drives the
# real Mac app through its debug server against this script's own daemon, in light and dark. Window
# shots go to $WHEEL_SHOTS (default $SCR/shots).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"

FIX="$SCR/usage.json"
# Resets in the future, so the pace tick and the clock have something to draw.
SOON="$(date -u -v+2H +%Y-%m-%dT%H:%M:%SZ)"
LATER_EPOCH="$(( $(date +%s) + 3 * 86400 ))"
LATER="$(date -u -r "$LATER_EPOCH" +%Y-%m-%dT%H:%M:%SZ)"
two_logins() {
  printf '{"accounts":[
    {"name":".claude","dirs":["/h/.claude"],"five_hour":{"utilization":42,"resets_at":"%s"},"seven_day":{"utilization":73.5,"resets_at":"%s"},"limits":[{"kind":"weekly_scoped","group":"weekly","percent":30,"resets_at":"%s","scope":{"model":{"display_name":"Fable"}}}],"billing":"Max 20x","email":"a@b.c"},
    {"name":".claude-work","dirs":["/h/.claude-work"],"five_hour":{"utilization":3,"resets_at":"%s"},"seven_day":{"utilization":86,"resets_at":"%s"},"billing":"Pro","email":"w@b.c"},
    {"name":".codex","agent":"codex","plan_type":"plus","email":"a@b.c","rate_limit":{"primary_window":{"used_percent":20,"limit_window_seconds":18000,"reset_at":%s},"secondary_window":{"used_percent":85,"limit_window_seconds":604800,"reset_at":%s}}},
    {"name":".codex-work","agent":"codex","plan_type":"prolite","email":"w@b.c","rate_limit":{"primary_window":{"used_percent":92,"limit_window_seconds":604800,"reset_at":%s},"secondary_window":null}}
  ]}\n' "$SOON" "$LATER" "$LATER" "$SOON" "$LATER" "$(( $(date +%s) + 7200 ))" "$LATER_EPOCH" "$LATER_EPOCH" >"$FIX"
}
one_login() {
  printf '{"accounts":[
    {"name":".claude","dirs":["/h/.claude"],"five_hour":null,"seven_day":{"utilization":90,"resets_at":"%s"},"limits":[{"kind":"weekly_all","group":"weekly","percent":90,"scope":null},{"kind":"weekly_scoped","group":"weekly","percent":41,"resets_at":"%s","scope":{"model":{"display_name":"Fable"}}}],"billing":"Ultra","email":"a@b.c"},
    {"name":".codex-work","agent":"codex","plan_type":"prolite","email":"w@b.c","rate_limit":{"primary_window":{"used_percent":92,"limit_window_seconds":604800,"reset_at":%s},"secondary_window":null}}
  ]}\n' "$LATER" "$LATER" "$LATER_EPOCH" >"$FIX"
}
two_logins

start_daemon --state-file "$SCR/spaces.json" --usage-fixture "$FIX"
prefs_reset
w config set usage.agents claude,codex --machine >/dev/null
w config set usage.labels folder,billing --machine >/dev/null
mkdir -p "$SCR/a"
w space new "$SCR/a" >/dev/null

# strip_rows <agent> <account>: the ids of the rows the strip draws for that account.
strip_rows() { st -c --arg a "$1" --arg n "$2" '[.usage.strip[] | select(.agent==$a and .account==$n) | .rows[] | .id]'; }
strip_labels() { st -c --arg a "$1" --arg n "$2" '[.usage.strip[] | select(.agent==$a and .account==$n) | .labels[]]'; }
shot() { app_post "/window/snapshot?path=$SHOTS/usage-$1.png" >/dev/null && echo "     shot $SHOTS/usage-$1.png"; }
shot_usage_window() { app_post "/window/snapshot?which=usage&path=$SHOTS/usage-$1.png" >/dev/null && echo "     shot $SHOTS/usage-$1.png"; }

for LOOK in light dark; do
  WHEEL_APPEARANCE=$LOOK launch_app
  check "$LOOK: the app connects" wait_st '.connection == "connected"' 20
  check "$LOOK: the board arrives with every login" wait_st '(.usage.strip | length) == 4' 20
  eq "$LOOK: .claude meters its 5h and weekly windows by default" "$(strip_rows claude .claude)" '["5h","weekly"]'
  eq "$LOOK: .codex-work has one weekly window, so the codex:5h meter draws nothing for it" "$(strip_rows codex .codex-work)" '["weekly"]'
  eq "$LOOK: .codex meters both of its windows" "$(strip_rows codex .codex)" '["5h","weekly"]'
  eq "$LOOK: two Claude logins: the folder and the billing print" "$(strip_labels claude .claude)" '[".claude","Max 20x"]'
  eq "$LOOK: two Codex logins: the folder and the plan print" "$(strip_labels codex .codex-work)" '[".codex-work","Prolite"]'
  eq "$LOOK: a meter row carries no pace or reset" "$(st -c '[.usage.strip[0].rows[0] | .level, .pace, .resets]')" '["meter",null,null]'
  shot "two-$LOOK"

  app_post "/usage?action=meter&row=claude:weekly&level=full" >/dev/null
  check "$LOOK: Full gives the weekly row a pace share and a reset clock" wait_st '[.usage.strip[] | select(.agent=="claude" and .account==".claude") | .rows[] | select(.id=="weekly")][0] | .level == "full" and (.pace | type) == "number" and (.resets | type) == "string"' 10
  echo "     $(st -c '[.usage.strip[] | select(.agent=="claude" and .account==".claude") | .rows[] | select(.id=="weekly")][0]')"
  eq "$LOOK: usage.meters holds the change in strip order" "$(w config get usage.meters)" "claude:5h:meter,claude:weekly:full,codex:5h:meter,codex:weekly:meter"
  shot "two-full-$LOOK"

  app_post "/usage?action=meter&row=claude:5h&level=off" >/dev/null
  check "$LOOK: Off takes the 5h row off every Claude login" wait_st '[.usage.strip[] | select(.agent=="claude") | .rows[] | select(.id=="5h")] | length == 0' 10
  app_post "/usage?action=meter&row=claude:5h&level=meter" >/dev/null
  check "$LOOK: Meter puts it back, last" wait_st '.usage.meters == ["claude:weekly:full","codex:5h:meter","codex:weekly:meter","claude:5h:meter"]' 10

  app_post "/usage?action=open" >/dev/null
  check "$LOOK: the Usage window opens" wait_st '.usage.window_open == true' 10
  eq "$LOOK: the window lists each window of .codex-work with its level" "$(st -c '[.usage.agents[] | select(.id=="codex") | .accounts[] | select(.name==".codex-work") | .rows[] | [.id, .level]]')" '[["weekly","meter"]]'
  eq "$LOOK: ...and .claude's three windows" "$(st -c '[.usage.agents[] | select(.id=="claude") | .accounts[] | select(.name==".claude") | .rows[] | [.id, .level]]')" '[["5h","meter"],["weekly","full"],["weekly-fable","off"]]'
  sleep 0.5
  shot_usage_window "window-two-$LOOK"
  app_post "/usage?action=close" >/dev/null

  app_post "/usage?action=label&label=email&on=true" >/dev/null
  check "$LOOK: the email label prints" wait_st '[.usage.strip[] | select(.agent=="claude" and .account==".claude") | .labels][0] == [".claude","Max 20x","a@b.c"]' 10
  app_post "/usage?action=label&label=email&on=false" >/dev/null
  app_post "/usage?action=label&label=billing&on=false" >/dev/null
  check "$LOOK: without the billing label the chip goes" wait_st '[.usage.strip[] | select(.agent=="claude" and .account==".claude") | .labels][0] == [".claude"]' 10
  app_post "/usage?action=label&label=billing&on=true" >/dev/null

  one_login
  app_post "/usage?action=refresh" >/dev/null
  check "$LOOK: one login per harness arrives" wait_st '(.usage.strip | length) == 2' 20
  eq "$LOOK: one Claude login: the folder does not print, the billing does" "$(strip_labels claude .claude)" '["Ultra"]'
  eq "$LOOK: an idle Claude login has no 5h row" "$(strip_rows claude .claude)" '["weekly"]'
  shot "one-$LOOK"

  w config set usage.meters claude:5h:meter,claude:weekly:meter,codex:5h:meter,codex:weekly:meter --machine >/dev/null
  two_logins
  w usage >/dev/null 2>&1 || true
  quit_app
  DEBUG_PORT=""
done

has_log "usage read account=.codex-work source=wham/usage via Codex logins windows=weekly=92%/604800s billing=Prolite" "log: the Codex read names its one window"
finish
