#!/usr/bin/env bash
# A mirror shows a pane of another session and closes with it; `wheel pane push` turns a pane's
# own pushes off and on, and the setting stays with the pane through a move.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

start_daemon

# leaf <pane> <field>: a field of pane <pane> from `status --json`, empty when absent.
leaf() {
  w status --json | python3 -c '
import json, sys
def walk(n):
    if isinstance(n, dict):
        if n.get("node") == "pane" and n.get("id") == sys.argv[1]:
            return n
        for v in n.values():
            if (f := walk(v)) is not None:
                return f
    elif isinstance(n, list):
        for v in n:
            if (f := walk(v)) is not None:
                return f
d = walk(json.load(sys.stdin).get("spaces", []))
print("" if d is None else str(d.get(sys.argv[2], "")).lower())' "$1" "$2"
}
first_pane() { w space ls --json | python3 -c 'import json,sys; print([s for s in json.load(sys.stdin) if s["id"]==sys.argv[1]][0]["panes"][0])' "$1"; }

HOME_SPACE="$(w space new /tmp)"
OTHER="$(w session new other)"
AWAY="$(w space new /tmp)"
w session move "$AWAY" "$OTHER" >/dev/null
SOURCE="$(first_pane "$AWAY")"
w pane start "$SOURCE" terminal >/dev/null

MIRROR="$(w pane mirror "$SOURCE" --space "$HOME_SPACE")"
check "a mirror of a pane in another session is made" test -n "$MIRROR"
eq "the mirror names its source" "$(leaf "$MIRROR" source)" "$SOURCE"
has_log "mirror made.*source=$SOURCE" "log: mirror made"

eq "a pane pushes until told otherwise" "$(leaf "$SOURCE" quiet)" ""
w pane push "$SOURCE" off >/dev/null
eq "push off makes the pane quiet" "$(leaf "$SOURCE" quiet)" "true"
w pane move "$SOURCE" --end-of "$HOME_SPACE" >/dev/null
eq "the pane stays quiet through a move" "$(leaf "$SOURCE" quiet)" "true"
w pane push "$SOURCE" on >/dev/null
eq "push on clears it" "$(leaf "$SOURCE" quiet)" ""
expect_exit 6 "push on a pane that does not exist is refused" w pane push p-none off

w pane close "$SOURCE" >/dev/null
eq "closing the source closes the mirror" "$(leaf "$MIRROR" id)" ""

finish
