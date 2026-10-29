#!/bin/sh
# Writes $WHEEL_DATA_DIR/review.json for the page: every night's suggestions from the Companion
# relay's chat companion:wheel-review (newest first), each with the verdict last flagged for it in
# wheeld's usage journal. Needs RELAY_URL and RELAY_API_KEY in the environment wheeld runs in, plus
# curl and jq. A failure is written to review.json as {"error": "..."} so the page shows it.
#
# A night's message `content` is a JSON object {"suggestions": [{"id", "title", "evidence"}]},
# bare or inside a ```json fence. `id` is the key a flag carries; a suggestion without one is
# keyed "<message id>.<position>". `evidence` is text (counts, timings, the action sequence).
out="${WHEEL_DATA_DIR:?WHEEL_DATA_DIR is not set}/review.json"
journal="${WHEEL_JOURNAL:-$WHEEL_DATA_DIR/../../usage.jsonl}"
chat="${WHEEL_REVIEW_CHAT:-companion:wheel-review}"
fail() { printf '{"error":%s}\n' "$(printf %s "$1" | jq -Rs .)" >"$out.tmp" && mv "$out.tmp" "$out"; exit 0; }

[ -n "$RELAY_URL" ] && [ -n "$RELAY_API_KEY" ] || fail "RELAY_URL and RELAY_API_KEY are not set in wheeld's environment"
rows=""
before=""
for _ in 1 2 3 4 5; do
  page="$(curl -fsS -m 15 -H "Authorization: Bearer $RELAY_API_KEY" "$RELAY_URL/api/companion/messages?limit=100${before:+&before=$before}")" \
    || fail "GET $RELAY_URL/api/companion/messages failed"
  rows="$rows$(printf %s "$page" | jq -c --arg chat "$chat" '.messages[] | select(.chat_id == $chat)')
"
  [ "$(printf %s "$page" | jq -r .has_more)" = true ] || break
  before="$(printf %s "$page" | jq -r .next_before)"
done

flags='[]'
[ -r "$journal" ] && flags="$(jq -cs '[.[] | select(.op == "review.flag")]' "$journal" 2>/dev/null || echo '[]')"

printf %s "$rows" | jq -cs --argjson flags "$flags" '
  ($flags | map({key: .id, value: {verdict, note, flagged_ms: .ts_ms}}) | from_entries) as $v
  | {nights: [ .[]
      | . as $m
      | ((.content | (capture("```json\\s*(?<j>[\\s\\S]*?)```").j // .) | fromjson? ) // {}) as $body
      | { message: .id, night: .created_at,
          suggestions: [ ($body.suggestions // []) | to_entries[]
            | .value + {id: (.value.id // ($m.id + "." + (.key + 1 | tostring)))} ] }
      | .suggestions |= map(. + ($v[.id] // {}))
      | select(.suggestions | length > 0) ]
      | sort_by(.night) | reverse}' >"$out.tmp" && mv "$out.tmp" "$out" || fail "could not read the relay's reply"
