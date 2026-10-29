#!/bin/bash
# Writes $WHEEL_DATA_DIR/sessions.json for the page: {"at": <unix s>, "sessions": [{"id", "project",
# "state"}]}, newest first, at most config.json's "max" long. "state" is "working", or "waiting" for a
# Wheel agent pane with an open permission request. Needs jq; the "wheel" source and a
# "session:<pane or session name>" source also need the wheel CLI (WHEEL_BIN, else wheel on PATH).
# A failure is written to sessions.json as an "error" next to the sessions found, so the page shows it.
#
# config.json "source":
#   all             every *.jsonl under "roots" written in the last 20 s; its project is the cwd field
#   wheel           agent panes with a turn running, or waiting on a permission request
#   session:<id>    one transcript (by session id) or one agent pane (by pane id or session name)
here="$(cd "$(dirname "$0")" && pwd)"
out="${WHEEL_DATA_DIR:?WHEEL_DATA_DIR is not set}/sessions.json"
cfg="${COMPANION_CONFIG:-$here/config.json}"
wheel="${WHEEL_BIN:-wheel}"
window=20
errors=""
note() { errors="${errors:+$errors; }$1"; }

source="$(jq -r '.source // "all"' "$cfg" 2>/dev/null)" || { source=all; note "config.json is unreadable"; }
max="$(jq -r '.max // 8' "$cfg" 2>/dev/null || echo 8)"
roots="$(jq -r '(.roots // ["$HOME/.claude/projects"])[]' "$cfg" 2>/dev/null || echo '$HOME/.claude/projects')"

rows=""   # one JSON object per line

transcripts() { # transcripts <only session id or empty>: rows for transcripts written in the window, newest first
  local ref only="$1" root file id project found=""
  ref="$(mktemp)"
  touch -t "$(date -v-"${window}"S +%Y%m%d%H%M.%S)" "$ref"
  while IFS= read -r root; do
    root="${root/#\$HOME/$HOME}"; root="${root/#\~/$HOME}"
    [ -d "$root" ] || continue
    # A session's subagents write <session>/subagents/agent-*.jsonl; they are not sessions of their own.
    found="$found$(find "$root" -name '*.jsonl' -not -path '*/subagents/*' -newer "$ref" -exec stat -f '%m %N' {} + 2>/dev/null)
"
  done <<<"$roots"
  rm -f "$ref"
  while IFS= read -r file; do
    [ -n "$file" ] || continue
    id="$(basename "$file" .jsonl)"
    [ -z "$only" ] || [ "$id" = "$only" ] || continue
    project="$(head -n 50 "$file" | jq -r '.cwd? // empty' 2>/dev/null | head -n 1)"
    [ -n "$project" ] || continue
    rows="$rows$(jq -nc --arg id "$id" --arg project "$project" '{id: $id, project: $project, state: "working"}')
"
  done < <(printf %s "$found" | sort -rn | cut -d' ' -f2-)
}

panes() { # panes <only pane or session name or empty>: rows for agent panes
  local status perms only="$1"
  status="$("$wheel" status --json 2>&1)" && perms="$("$wheel" perm list 2>&1)" \
    && printf %s "$status" | jq -e . >/dev/null 2>&1 && printf %s "$perms" | jq -e . >/dev/null 2>&1 \
    || { note "wheel status or perm list failed"; return 1; }
  rows="$rows$(jq -nc --argjson status "$status" --argjson perms "$perms" --arg only "$only" '
    [ $perms[].session ] as $asking
    | [ $status.spaces[] | .cwd as $cwd | .root | .. | objects | select(.kind == "agent")
        | select($only == "" or .id == $only or .session == $only)
        | { id: .session, project: $cwd,
            state: (if ([.session] | inside($asking)) then "waiting"
                    elif .state.phase == "busy" then "working" else empty end) } ]
    | .[]')
"
  return 0
}

case "$source" in
  all) transcripts "" ;;
  wheel) panes "" ;;
  session:*)
    target="${source#session:}"
    before="$errors"
    panes "$target" || true
    if [ -z "$rows" ]; then
      transcripts "$target"
      [ -z "$rows" ] || errors="$before" # the transcript answers; the pane lookup failing is not news
    fi
    ;;
  *) note "source $source is not all, wheel or session:<id>" ;;
esac

printf %s "$rows" | jq -cs --argjson at "$(date +%s)" --argjson max "$max" --arg error "$errors" '
  { at: $at, sessions: (reduce .[] as $s ([]; if any(.[]; .id == $s.id) then . else . + [$s] end) | .[0:$max]) }
  + (if $error == "" then {} else {error: $error} end)' >"$out.tmp" && mv "$out.tmp" "$out"
