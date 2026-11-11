#!/usr/bin/env bash
# § Wheelhouse list actions in the Mac app (wheel-xxp1): the Beads page's Copy ids button, pressed
# by id through the hidden Debug app's `/a2ui/action`, puts the listed ids on the pasteboard; a sort
# the page is told (`set_sort`) redraws the rows in the page's order, with no sorting in the client.
# Saves and restores the person's pasteboard around the press. Needs `make build-client`,
# `cargo build -p wheelhouse -p wheeld -p wheel-cli`, bd, jq. Writes a picture of the page to
# $WHEEL_SHOT when that is set.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HOUSE="$BIN/wheelhouse"
[ -x "$HOUSE" ] || { echo "missing $HOUSE: run cargo build -p wheelhouse" >&2; exit 2; }
command -v bd >/dev/null || { echo "bd is not installed" >&2; exit 2; }

start_daemon --state-file "$SCR/spaces.json"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
export WHEEL_URL="ws://127.0.0.1:$PORT/ws" WHEEL_TOKEN_FILE="$INST/token"

PROJ="$SCR/proj"; mkdir -p "$PROJ"
(cd "$PROJ" && bd init --prefix lm --quiet >/dev/null 2>&1 \
  && bd create "Alpha" -t task -p 3 -l cli >/dev/null 2>&1 \
  && bd create "Beta" -t task -p 1 -l cli >/dev/null 2>&1)
A="$(cd "$PROJ" && bd list --json | jq -r '.[] | select(.title=="Alpha") | .id')"
B="$(cd "$PROJ" && bd list --json | jq -r '.[] | select(.title=="Beta") | .id')"
printf '[beads.buttons.iterate-list]\nlabel = "Iterate"\non = ["list"]\nrun = "true"\n' >"$PROJ/wheel.toml"
printf '[artifacts.beads]\nmcp = "exec %s serve"\nshared = true\ntitle = "Beads"\n' "$HOUSE" >>"$INST/wheel.toml"

SP="$(w space new "$PROJ")"
PANE="$(w space ls | jq -r --arg s "$SP" '.spaces[] | select(.id==$s) | [.root | .. | select(.node? == "pane")][0].id')"
w pane kind "$PANE" web --artifact beads --scope machine >/dev/null

launch_app
trap 'app_cleanup; cleanup' EXIT
drawn() { app_post "/a2ui/action?pane=$PANE&name=get_beads" >/dev/null 2>&1; }
check "the app draws the Beads page" poll 30 drawn

BEFORE="$(pbpaste 2>/dev/null || true)"
sleep 1
check "Copy ids can be pressed by id" app_post "/a2ui/action?pane=$PANE&id=list.copy"
sleep 0.5
eq "the pasteboard holds the two listed ids" "$(pbpaste | tr ' ' '\n' | sort | tr '\n' ' ')" "$(printf '%s\n%s\n' "$A" "$B" | sort | tr '\n' ' ')"
check "a component the page lacks is refused" sh -c "! curl -sf -X POST -H 'Authorization: Bearer $(TOKEN)' 'http://127.0.0.1:$DEBUG_PORT/a2ui/action?pane=$PANE&id=nothing' >/dev/null"
printf %s "$BEFORE" | pbcopy

# The page is told a sort; the client redraws in the page's order.
app_post "/a2ui/action?pane=$PANE&name=set_sort&key=pri&asc=true" >/dev/null
sleep 1
app_post "/a2ui/action?pane=$PANE&id=list.copy" >/dev/null
sleep 0.5
eq "after a sort the pasteboard holds the ids in the page's new order" "$(pbpaste)" "$B $A"
printf %s "$BEFORE" | pbcopy
if [ -n "${WHEEL_SHOT:-}" ]; then
  app_post "/window/snapshot?path=$WHEEL_SHOT" >/dev/null && echo "picture: $WHEEL_SHOT"
fi

finish
