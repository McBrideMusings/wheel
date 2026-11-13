#!/usr/bin/env bash
# § Pairing: one-time tokens from the CLI, the paired-device list and revoke. The remote listener's
# refusals (a used token, an expired one, the token file's token) are covered by the committed
# tests in daemon/tests/remote.rs, which speak TLS; this drives what the CLI can.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

RPORT="$(pick_port)"
printf '[remote]\nlisten = "127.0.0.1:%s"\n' "$RPORT" >"$INST/wheel.toml"
start_daemon

CODE="$(w remote pair --address 127.0.0.1)"
eq "pair prints a pairing code with the listener's address" "$(printf %s "$CODE" | jq -r .url)" "wss://127.0.0.1:$RPORT/ws"
eq "the code carries the certificate fingerprint" "$(printf %s "$CODE" | jq -r '.cert_sha256 | length')" "64"
eq "the code carries a 64-hex token" "$(printf %s "$CODE" | jq -r '.token | length')" "64"
check "the token is not the token file's" test "$(printf %s "$CODE" | jq -r .token)" != "$(TOKEN)"
CODE2="$(w remote pair --address 127.0.0.1)"
check "every code carries its own token" test "$(printf %s "$CODE2" | jq -r .token)" != "$(printf %s "$CODE" | jq -r .token)"
expect_exit 1 "an address the daemon does not listen on is refused" w remote pair --address 10.9.9.9
expect_out "no paired devices" "nothing has paired yet" w remote devices
eq "devices --json is an empty list" "$(w remote devices --json | jq length)" "0"
expect_exit_out 6 "no paired device d9" "revoking a device nobody paired exits 6" w remote revoke d9
has_log "pairing token issued" "the log says a token was issued"
finish
