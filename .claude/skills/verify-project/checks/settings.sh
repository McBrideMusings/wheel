#!/usr/bin/env bash
# § 2d Settings: the three origins, refusals that leave the file alone, hand-written bad values,
# unset, the lead.agent ACP probe, the settings broadcast.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

STATE="$SCR/spaces.json"
P1="$SCR/proj1"; P2="$SCR/proj2"
mkdir -p "$P1" "$P2"
start_daemon --no-lead --state-file "$STATE"

hp() { w config ls ${1:+--space "$1"} --json; }
origin_of() { hp "$1" | jget 0.origin; }
value_of() { hp "$1" | jget 0.value; }

# no space at all
expect_exit 0 "config ls answers with no space" w config ls
expect_exit_out 6 'no space to resolve a project setting against' "config set without --machine and no space exits 6" w config set browser.homepage https://x.example
expect_exit 6 "--space s999 on set exits 6" w config set browser.homepage https://x.example --space s999
expect_exit_out 6 'no space s999' "...naming the space" w config set browser.homepage https://x.example --space s999

w space new "$P1" >/dev/null
S2="$(w space new "$P2")"

# the three origins
eq "default: about:blank" "$(value_of s1)" "about:blank"
eq "default: origin default" "$(origin_of s1)" "default"
expect_exit 0 "set --machine" w config set browser.homepage https://machine.example --machine
check "...writes <inst>/wheel.toml" grep -q 'https://machine.example' "$INST/wheel.toml"
check "...and the project directory gains nothing" test ! -e "$P1/wheel.toml"
eq "s1: origin machine" "$(origin_of s1)" "machine"
eq "s2: origin machine" "$(origin_of "$S2")" "machine"
expect_exit 0 "set --space s1" w config set browser.homepage https://project.example --space s1
check "...writes <cwd>/wheel.toml" grep -q 'https://project.example' "$P1/wheel.toml"
eq "s1: origin project" "$(origin_of s1)" "project"
eq "s1: value is the project's" "$(value_of s1)" "https://project.example"
eq "s2 still reports the machine value" "$(value_of "$S2")" "https://machine.example"
expect_out "this project's wheel.toml" "plain config ls names the file the value came from" w config ls --space s1
expect_out "this machine's: https://machine.example" "...and the machine value it hides" w config ls --space s1
eq "config get prints the bare value" "$(w config get browser.homepage --space s1)" "https://project.example"

# a write keeps the rest of the file
printf 'other = 1\n' >"$P2/wheel.toml"
w config set browser.homepage https://two.example --space "$S2" >/dev/null
check "other = 1 survives the set" grep -qx 'other = 1' "$P2/wheel.toml"
w config set browser.homepage https://two.example --space "$S2" >/dev/null
eq "a second set leaves one value" "$(grep -c homepage "$P2/wheel.toml")" "1"

# every refusal leaves the file byte-identical
BEFORE="$(cat "$P1/wheel.toml")"
for v in example.com file://otherhost/etc/passwd 'javascript:alert(1)' about:config ''; do
  expect_exit_out 6 'http:// or https:// URL with a host, a file:/// URL, or about:blank' "set '$v' exits 6 naming what the key takes" w config set browser.homepage "$v" --space s1
done
eq "refusals leave the project file byte-identical" "$(cat "$P1/wheel.toml")" "$BEFORE"
expect_exit_out 6 "Wheel's settings are: browser.homepage" "an unknown key exits 6 naming the keys that exist" w config set browser.homepge x --space s1
expect_exit 6 "config get on an unknown key exits 6" w config get nosuch.key

# a hand-written bad value is ignored and reported
printf '[browser]\nhomepage = "javascript:alert(1)"\n' >"$P1/wheel.toml"
eq "bad hand-written value: the machine's is in force" "$(value_of s1)" "https://machine.example"
eq "...origin machine" "$(origin_of s1)" "machine"
expect_out 'problem' "...and plain config ls reports a problem" w config ls --space s1
check "the problem names the file" bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' config ls --space s1 --json | grep -q 'proj1/wheel.toml'"
printf '[browser]\nhomepage = 5\n' >"$P1/wheel.toml"
expect_out 'not a string' "homepage = 5 is reported as not a string" w config ls --space s1
printf 'browser = 1\n' >"$P1/wheel.toml"
BEFORE="$(cat "$P1/wheel.toml")"
expect_exit_out 6 'is not a table' "a key whose parent is not a table exits 6" w config set browser.homepage https://x.example --space s1
eq "...and leaves the file as it was" "$(cat "$P1/wheel.toml")" "$BEFORE"

# a file that is not TOML sets nothing and is never overwritten
printf 'this is = = not toml\n' >"$P1/wheel.toml"
eq "a damaged project file: the machine layer is in force" "$(origin_of s1)" "machine"
expect_exit_out 6 'is not valid TOML' "a write into a damaged file exits 6" w config set browser.homepage https://x.example --space s1
eq "...leaving it as it was" "$(cat "$P1/wheel.toml")" "this is = = not toml"
printf '[browser]\nhomepage = "https://project.example"\n' >"$P1/wheel.toml"

# settings are files, not layout
check "grep homepage the state file finds nothing" file_lacks homepage "$STATE"
restart_daemon
eq "a restart reports the same value" "$(value_of s1)" "https://project.example"
eq "...and the same origin" "$(origin_of s1)" "project"
expect_exit_out 6 "no space s999" "config ls --space s999 exits 6" w config ls --space s999
has_log "setting written" "log: setting written"

# unset
printf 'keep = 1\n[browser]\nhomepage = "https://project.example"\n' >"$P1/wheel.toml"
expect_exit_out 0 'origin|machine' "unset exits 0" w config unset browser.homepage --space s1
eq "...the machine value is in force" "$(origin_of s1)" "machine"
eq "...the project's other keys stay" "$(cat "$P1/wheel.toml")" "keep = 1"
SNAP="$(cat "$P1/wheel.toml")"
expect_exit 0 "a second unset exits 0" w config unset browser.homepage --space s1
eq "...leaving the file byte-identical" "$(cat "$P1/wheel.toml")" "$SNAP"
expect_exit 0 "unset --machine" w config unset browser.homepage --machine
eq "...leaves about:blank by default" "$(origin_of s1)" "default"
expect_exit_out 6 "Wheel's settings are" "unset of an unknown key exits 6 naming the keys" w config unset nosuch.key
printf 'not = = toml\n' >"$P1/wheel.toml"
expect_exit_out 6 'is not valid TOML' "unset on a damaged file exits 6" w config unset browser.homepage --space s1
printf 'keep = 1\n' >"$P1/wheel.toml"
has_log "setting removed.*removed=true" "log: setting removed removed=true"
has_log "setting removed.*removed=false" "log: setting removed removed=false"

# lead.agent takes only a command that answers ACP initialize
expect_exit 0 "lead.agent = the fake agent" w config set lead.agent "$FAKE_AGENT" --machine
check "...written to <inst>/wheel.toml" grep -q "agent = \"$FAKE_AGENT\"" "$INST/wheel.toml"
SNAP="$(cat "$INST/wheel.toml")"
expect_exit_out 6 'does not speak ACP: no answer to initialize within 5s' "lead.agent = 'sleep 30' exits 6 after the probe" w config set lead.agent "sleep 30" --machine
expect_exit_out 6 'exited, or closed its output, without answering initialize' "lead.agent = a command that exits at once" w config set lead.agent nosuchcmd-xyz --machine
eq "each refusal leaves the file byte-identical" "$(cat "$INST/wheel.toml")" "$SNAP"
w config unset lead.agent --machine >/dev/null

# the settings broadcast
WOUT="$SCR/watch.out"
sleep 1; bg bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' config watch --timeout 20 >'$WOUT' 2>&1"
WP=$BGPID
listening() { grep -q 'watching settings' "$WOUT"; }
poll 10 listening
w config set supervisor.cap 40 --machine >/dev/null
wait "$WP" 2>/dev/null
check "config watch prints settings space=" grep -q '^settings space=' "$WOUT"
check "...and the value with its origin" grep -q "supervisor.cap = 40   (machine)" "$WOUT"
sleep 1; bg bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' config watch --timeout 20 >'$WOUT' 2>&1"
WP=$BGPID
poll 10 listening
w config set browser.homepage https://project.example --space s1 >/dev/null
wait "$WP" 2>/dev/null
check "a project write is broadcast with space=s1" grep -q '^settings space=s1' "$WOUT"
check "...and (project)" grep -q '(project)' "$WOUT"
expect_exit 3 "config watch with no write exits 3" w config watch --timeout 0.5

finish
