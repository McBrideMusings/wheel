#!/usr/bin/env bash
# § 2e Title-row buttons (ADR-0037): `[[title.button]]` entries of this machine's wheel.toml and the
# space's, merged by id; `state` output laid over the entry; `command` run on a press in the space's
# directory; hidden entries; bad entries skipped; a list change pushed without a client asking.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

P1="$SCR/proj1"; mkdir -p "$P1"
start_daemon --state-file "$SCR/spaces.json"
S="$(w space new "$P1")"

cat >"$INST/wheel.toml" <<'EOF'
[[title.button]]
id = "vpn"
icon = "lock"
tip = "VPN"
command = "echo machine-pressed > pressed.txt"
state = "echo '{\"icon\":\"lock.fill\",\"tint\":\"green\",\"badge\":\"2\"}'"
every = 1

[[title.button]]
id = "pin"
hidden = true

[[title.button]]
icon = "star"

[[title.button]]
id = "odd"
icon = "star"
side = "middle"
EOF

ids() { w title ls --space "$S" --json | grep -o '"id": "[a-z]*"' | sed 's/.*: //;s/"//g' | tr '\n' ' '; }
field() { w title ls --space "$S" --json | jget "$1"; }

poll 5 test "$(field 0.icon)" = lock.fill
eq "a machine entry lists, with the icon its state printed" "$(field 0.icon)" "lock.fill"
eq "...its tint" "$(field 0.tint)" "green"
eq "...its badge" "$(field 0.badge)" "2"
eq "...its tip from the entry" "$(field 0.tip)" "VPN"
eq "...its origin" "$(field 0.origin)" "machine"
eq "a hidden entry lists as hidden" "$(field 1.hidden)" "true"
eq "an entry with no id and one with side middle are skipped" "$(ids)" "vpn pin "
has_log 'a \[\[title.button\]\] entry has no id; it is skipped' "the log names the skipped entry"
has_log 'side is left or right' "...and the bad side"

cat >"$P1/wheel.toml" <<'EOF'
[[title.button]]
id = "vpn"
icon = "lock.open"
side = "right"
tip = "Project VPN"
command = "echo project-pressed > pressed.txt"

[[title.button]]
id = "deploy"
icon = "shippingbox"
EOF

poll 5 test "$(field 0.origin)" = project
eq "the project's entry replaces the machine's by id, in place" "$(ids)" "vpn pin deploy "
eq "...with its icon (no state, so the entry's own)" "$(field 0.icon)" "lock.open"
eq "...its side" "$(field 0.side)" "right"
eq "...its origin" "$(field 0.origin)" "project"
eq "an entry with no command reports has_command false" "$(field 2.has_command)" "false"

expect_exit 0 "press runs the command" w title press vpn --space "$S"
poll 5 test -s "$P1/pressed.txt"
eq "...in the space's directory, the project's command" "$(cat "$P1/pressed.txt")" "project-pressed"
expect_exit_out 6 'has no command' "press of a button with no command exits 6" w title press deploy --space "$S"
expect_exit_out 6 'no title button nope' "press of an unknown id exits 6" w title press nope --space "$S"
expect_exit_out 6 'no space s999' "press in an unknown space exits 6" w title press vpn --space s999

# a project's state never runs: no timer-run command from a repository
printf '[[title.button]]\nid = "sneaky"\nicon = "bolt"\ncommand = "true"\nstate = "echo {} > ran-state.txt; echo {}"\nevery = 1\n' >"$P1/wheel.toml"
poll 5 sh -c "w title ls --space $S --json | grep -q sneaky"
sleep 3
check "a project entry's state command never runs" test ! -e "$P1/ran-state.txt"

# a state that hangs is killed with everything it started, and a disabled button refuses a press
cat >"$INST/wheel.toml" <<'EOF'
[[title.button]]
id = "slow"
icon = "bolt"
command = "true"
state = "sleep 47 & sleep 47; echo {}"
every = 1

[[title.button]]
id = "off"
icon = "bolt"
command = "true"
state = "echo '{\"enabled\":false}'"
every = 1
EOF
sleep 16
check "a state past its 5 s limit leaves nothing behind (3 cycles; one live run is 3 processes)" sh -c '[ "$(pgrep -f "sleep 47" | wc -l)" -le 3 ]'
expect_exit_out 6 'is disabled' "a press of a button whose state printed enabled false exits 6" w title press off --space "$S"

# a list change is pushed to a client that only listens
LISTEN="$SCR/listen.out"
bg sh -c "node -e '
const ws=new WebSocket(\"ws://127.0.0.1:$PORT/ws?token=\"+require(\"fs\").readFileSync(\"$INST/token\",\"utf8\").trim());
ws.onmessage=e=>{const m=JSON.parse(e.data);if(m.ev===\"title_buttons_changed\")console.log(e.data)};
setTimeout(()=>process.exit(0),8000)' >\"$LISTEN\" 2>&1"
sleep 1
printf '[[title.button]]\nid = "late"\nicon = "bolt"\n' >"$P1/wheel.toml"
poll 6 grep -q '"id":"late"' "$LISTEN"
check "a changed file is pushed as title_buttons_changed with no request" grep -q '"id":"late"' "$LISTEN"
has_log 'title buttons changed' "the log has title buttons changed"

finish
