#!/usr/bin/env bash
# § 2c-usage-review The usage-review artifact (wheel-fl9q): its refresh command reads the nightly
# suggestions from a fake relay's chat companion:wheel-review into review.json, a page's flag
# (POST .wheel/review-flag, as wheel.flag sends it) lands in usage.jsonl as op=review.flag, and
# the next refresh shows that verdict on the suggestion. The journal's `op` becomes the Loki
# `kind` label (logship.rs); shipping is not run here.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
command -v jq >/dev/null || { echo "this check needs jq" >&2; exit 2; }
command -v python3 >/dev/null || { echo "this check needs python3 (the fake relay)" >&2; exit 2; }

RPORT="$(pick_port)"
cat >"$SCR/relay.py" <<'PY'
import http.server, json, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(s):
        if s.headers['Authorization'] != 'Bearer relay-key':
            s.send_response(401); s.end_headers(); return
        body = {"has_more": False, "messages": [
          {"id": "9", "chat_id": "main", "content": "unrelated", "created_at": "2026-10-28T03:00:00Z"},
          {"id": "8", "chat_id": "companion:wheel-review", "created_at": "2026-10-28T03:00:00Z",
           "content": "Tonight:\n```json\n{\"suggestions\":[{\"id\":\"n8-1\",\"title\":\"Alias git status\",\"evidence\":\"ran 14 times\"},{\"title\":\"No id\",\"evidence\":\"e\"}]}\n```"},
          {"id": "7", "chat_id": "companion:wheel-review", "created_at": "2026-10-27T03:00:00Z",
           "content": "{\"suggestions\":[{\"id\":\"n7-1\",\"title\":\"Older\",\"evidence\":\"x\"}]}"}]}
        s.send_response(200); s.send_header('content-type', 'application/json'); s.end_headers()
        s.wfile.write(json.dumps(body).encode())
    def log_message(*a): pass
http.server.HTTPServer(('127.0.0.1', int(sys.argv[1])), H).serve_forever()
PY
python3 "$SCR/relay.py" "$RPORT" &
BG_PIDS+=("$!")
check "the fake relay listens" poll 50 bash -c "(exec 3<>/dev/tcp/127.0.0.1/$RPORT) 2>/dev/null"

mkdir -p "$SCR/proj"
printf '[artifacts.usage-review]\npath = "%s/artifacts/usage-review"\ncommand = "%s/artifacts/usage-review/refresh.sh"\nevery = 3600\n' "$REPO" "$REPO" >>"$INST/wheel.toml"
RELAY_URL="http://127.0.0.1:$RPORT" RELAY_API_KEY=relay-key start_daemon --state-file "$SCR/spaces.json"
w space new "$SCR/proj" >/dev/null
P="$(w space ls | jget spaces.0.root.id)"
B="$(BASE)/artifacts/$P"
DATA="$INST/panes/$P/review.json"
expect_exit 0 "a web pane opens it" w pane kind "$P" web --artifact usage-review --scope machine
eq "the page is served: 200" "$(curl -s -o /dev/null -w '%{http_code}' -L -c "$SCR/jar" "$B/index.html?token=$(TOKEN)")" "200"
check "and talks to the page channel" sh -c "curl -s -b '$SCR/jar' '$B/' | grep -q 'wheel.flag'"

w artifact refresh "$P" >/dev/null
check "the refresh wrote review.json" poll 20 test -s "$DATA"
eq "the newest night comes first" "$(jq -r '.nights[0].message' "$DATA")" "8"
eq "only the review chat's messages are read" "$(jq -r '.nights | length' "$DATA")" "2"
eq "a suggestion without an id is keyed by message and position" "$(jq -r '.nights[0].suggestions[1].id' "$DATA")" "8.2"
eq "evidence reaches the page" "$(jq -r '.nights[0].suggestions[0].evidence' "$DATA")" "ran 14 times"

flag() { curl -s -o /dev/null -w '%{http_code}' -b "$SCR/jar" -H 'content-type: application/json' -d "$1" "$B/.wheel/review-flag"; }
eq "a verdict that is not good or bad is refused: 400" "$(flag '{"id":"n8-1","verdict":"meh"}')" "400"
eq "a bad flag with a note is taken: 204" "$(flag '{"id":"n8-1","verdict":"bad","note":"I already alias it"}')" "204"
check "usage.jsonl holds the review.flag line" poll 20 sh -c "jq -e 'select(.op == \"review.flag\" and .id == \"n8-1\" and .verdict == \"bad\")' '$INST/usage.jsonl'"
echo "     $(grep review.flag "$INST/usage.jsonl")"

w artifact refresh "$P" >/dev/null
check "the next refresh shows the verdict" poll 20 sh -c "[ \"\$(jq -r '.nights[0].suggestions[0].verdict' '$DATA')\" = bad ]"
eq "and its note" "$(jq -r '.nights[0].suggestions[0].note' "$DATA")" "I already alias it"

finish
