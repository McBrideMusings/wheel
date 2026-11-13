#!/usr/bin/env bash
# § New Space mode of the command palette (wheel-a10q.2): open with mode=new_space, name and path
# queries, tab, an open folder, a missing path, running a folder. Drives the real Mac app through
# its debug server against this script's own daemon, whose home directory is a fixture.
# WHEEL_SHOTS=<dir> keeps the window snapshots (set WHEEL_APPEARANCE=light|dark for the theme).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT
SHOTS="${WHEEL_SHOTS:-$SCR/shots}"
mkdir -p "$SHOTS"
LOOK="${WHEEL_APPEARANCE:-light}"

FAKEHOME="$SCR/home"
mkdir -p "$FAKEHOME/Projects/wheel" "$FAKEHOME/Projects/wharf" "$FAKEHOME/Projects/notes"
use_fake_agent
HOME="$FAKEHOME" start_daemon --state-file "$SCR/spaces.json"
w config set --machine spaces.roots "~/Projects" >/dev/null
prefs_reset
launch_app
wait_st '.windows | length == 1' 20

shot() { app_post "/window/snapshot?path=$SHOTS/new-space-$1-$LOOK.png" >/dev/null && echo "     shot $SHOTS/new-space-$1-$LOOK.png"; }
rows() { st -c '[.palette.rows[] | .kind + ":" + .title]'; }
palette() { app_post "/palette?$1" >/dev/null; }
settle() { sleep 0.6; }

palette "action=open&mode=new_space"
settle
eq "the palette opens in new_space mode" "$(st .palette.mode)" "new_space"
eq "an empty field lists the root's folders, Choose in Finder… last" "$(rows)" \
  '["folder:notes","folder:wharf","folder:wheel","open_folder:Choose in Finder…"]'
shot empty

palette "action=type&text=whe"
settle
eq "a name with no path finds the folder under the root" "$(rows)" '["folder:wheel","open_folder:Choose in Finder…"]'
eq "its row names the absolute path" "$(st '.palette.rows[0].detail')" "$FAKEHOME/Projects/wheel"
shot name

palette "action=type&text=~/Projects/wh"
settle
eq "a path query lists that folder's subfolders matching the rest" "$(st -c "[.palette.rows[].title]")" '["wharf","wheel","Choose in Finder…"]'
shot path
palette "action=select&index=1"
palette "action=tab"
settle
eq "tab completes the query with the way it was typed" "$(st .palette.query)" "~/Projects/wheel/"

# An open folder says so, and running it shows that space.
SP="$(w space new "$FAKEHOME/Projects/wharf")"
wait_st '.spaces | length >= 1' 10
palette "action=type&text=~/Projects/wha"
settle
eq "a folder that is open says so" "$(st '.palette.rows[0].detail')" "open · $FAKEHOME/Projects/wharf"
BEFORE="$(w space ls | jq '.spaces | length')"
palette "action=run"
settle
eq "running an open folder opens no second space" "$(w space ls | jq '.spaces | length')" "$BEFORE"
eq "and closes the palette" "$(st .palette.open)" "false"

# A missing path.
palette "action=open&mode=new_space"
palette "action=type&text=~/Projcts/wh"
settle
eq "a missing path says so, with Choose in Finder… below" "$(rows)" '["missing:No folder at ~/Projcts/","open_folder:Choose in Finder…"]'
shot missing

# Running a folder that is not open makes its space.
palette "action=type&text=~/Projects/wheel"
settle
palette "action=run"
wait_st '.palette.open == false' 5
settle
eq "running a closed folder creates a space for it" "$(w space ls | jq -r '[.spaces[].cwd] | map(select(endswith("/Projects/wheel"))) | length')" "1"
NEWSP="$(w space ls | jq -r '[.spaces[] | select(.cwd | endswith("/Projects/wheel")) | .id][0]')"
eq "and the focused window shows that new space" "$(st '[.windows[] | select(.focused)][0].active')" "$NEWSP"

finish
