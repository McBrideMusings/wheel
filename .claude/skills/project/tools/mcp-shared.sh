#!/usr/bin/env bash
# § Shared MCP sessions (ADR-0028, wheel-cmbu.1): shared machine entries that run the same command
# share one process; an entry with another command gets its own; changing an entry's command ends
# the server no entry names any more. Needs `cargo build --examples`. No app.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

SRV="$BIN/examples/mcp_test_server"
[ -x "$SRV" ] || { echo "missing $SRV: run cargo build --examples" >&2; exit 2; }

start_daemon --state-file "$SCR/spaces.json"
PROJ="$SCR/proj"
mkdir -p "$PROJ"
w space new "$PROJ" >/dev/null
SP="$(w space ls | jget spaces.0.id)"

# Three machine entries: a and b run one command, c another.
write_entries() {
  {
    printf '[artifacts.a]\nmcp = "exec %s stdio"\nshared = true\ntitle = "a"\n' "$SRV"
    printf '[artifacts.b]\nmcp = "exec %s stdio"\nshared = true\ntitle = "b"\n' "$SRV"
    printf '[artifacts.c]\nmcp = "VIEW=c exec %s stdio"\nshared = true\ntitle = "c"\n' "$SRV"
  } >"$INST/wheel.toml"
}
write_entries

# The daemon's own children running the server: the processes its sessions start.
servers() { pgrep -P "$DPID" -f "$SRV stdio" | wc -l | tr -d ' '; }
call() { w artifact call "$1" --scope machine --space "$SP" echo '{"text":"hi"}' >/dev/null 2>&1; }

check "a calls its server" call a
eq "one server runs after one entry's call" "$(servers)" 1
check "b, with the same command, calls it" call b
eq "a and b share one process" "$(servers)" 1
check "c, with another command, calls its own" call c
eq "c's server is a second process" "$(servers)" 2

# Editing b's command: the old server stays while a still names it.
sed -i '' "/^\[artifacts.b\]/,/^title/ s|exec $SRV stdio|VIEW=b exec $SRV stdio|" "$INST/wheel.toml"
check "b, with a new command, calls" call b
eq "b's new command is a third process; a's server stays" "$(servers)" 3

# Editing a's command to match none: the server nobody names ends.
sed -i '' "/^\[artifacts.a\]/,/^title/ s|exec $SRV stdio|VIEW=a exec $SRV stdio|" "$INST/wheel.toml"
check "a, with a new command, calls" call a
eq "the server no entry names ends: c, b and a's new one remain" "$(servers)" 3

has_log "MCP session no entry names any more" "the log says an unused session ended"
finish
