#!/usr/bin/env bash
# § 2c-builtins Built-in artifacts (wheel-9gxt.2): `wheel artifact builtin ls|install|update|remove`
# against a running wheeld, the machine entry an install writes, and a web pane serving the page.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

start_daemon --state-file "$SCR/spaces.json"
mkdir -p "$SCR/proj"
w space new "$SCR/proj" >/dev/null
P="$(w space ls | jget spaces.0.root.id)"
B="$(BASE)/artifacts/$P"
DIR="$INST/artifacts/sample"
http() { curl -s -o /dev/null -w '%{http_code}' "$@"; }

expect_out '^sample  shipped 1  not installed$' "ls lists the sample, not installed" w artifact builtin ls
expect_exit_out 6 'no built-in artifact nope: wheeld carries sample' "an unknown built-in is named" w artifact builtin install nope

expect_out '^sample  shipped 1  installed 1$' "install prints it installed" w artifact builtin install sample
check "the page is in the machine artifacts folder" test -f "$DIR/index.html"
check "builtin.toml is not installed" test ! -e "$DIR/builtin.toml"
expect_out "^machine sample" "artifact show --scope machine finds it" w artifact show sample --scope machine
expect_out "$DIR/index.html" "and resolves its page there" w artifact show sample --scope machine
eq "installed.json records version 1" "$(plutil -extract sample.version raw -o - "$INST/installed.json")" "1"
eq "and the page's sha256" "$(jq -r '.sample.files["index.html"]' "$INST/installed.json")" "$(shasum -a 256 "$DIR/index.html" | cut -d' ' -f1)"
expect_exit_out 6 'already installed' "a second install is refused" w artifact builtin install sample

expect_exit 0 "a web pane opens it" w pane kind "$P" web --artifact sample --scope machine
eq "and serves its page: 200" "$(http -L -c "$SCR/jar" "$B/index.html?token=$(TOKEN)")" "200"
check "the page is the sample's" sh -c "curl -s -b '$SCR/jar' '$B/' | grep -q 'Sample built-in'"

printf '<p>mine</p>\n' >"$DIR/index.html"
expect_out '^  edited: index.html$' "ls reports the edit" w artifact builtin ls
expect_exit_out 1 'has 1 edited file' "update with no choice exits 1" w artifact builtin update sample
expect_out '^-<p>mine</p>$' "and prints the diff" w artifact builtin update sample
eq "and changed nothing" "$(cat "$DIR/index.html")" "<p>mine</p>"
expect_exit 0 "update --keep-edited" w artifact builtin update sample --keep-edited
eq "keeps the edit" "$(cat "$DIR/index.html")" "<p>mine</p>"
expect_exit 0 "update --replace-edited" w artifact builtin update sample --replace-edited
check "puts the shipped page back" grep -q 'Sample built-in' "$DIR/index.html"
expect_out '^sample  shipped 1  installed 1$' "with nothing edited" w artifact builtin ls

expect_out '^sample  shipped 1  not installed$' "remove" w artifact builtin remove sample
check "the folder is gone" test ! -e "$DIR"
expect_exit 6 "the machine entry is gone" w artifact show sample --scope machine

mkdir -p "$DIR"
expect_exit_out 6 'Wheel did not install it' "a folder Wheel did not make is never installed over" w artifact builtin install sample
check "and is left as it was" test -d "$DIR"

has_log 'built-in installed name=sample version=1'
has_log 'built-in update asks about edited files'
has_log 'built-in updated name=sample'
has_log 'built-in removed name=sample'
has_log 'built-in request refused'
finish
