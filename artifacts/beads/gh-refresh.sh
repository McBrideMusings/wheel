#!/bin/sh
# Refresh command for a GitHub-issues repo: writes $WHEEL_DATA_DIR/beads.json for the repo `gh` resolves
# from the current directory, or for $1 given as owner/name.
set -e
here=$(dirname "$0")
repo=${1:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
gh issue list -R "$repo" --state all --limit 1000 --json number,title,state,labels,body,createdAt,closedAt > "$tmp/issues.json"
gh api graphql --paginate -f owner="${repo%%/*}" -f name="${repo#*/}" -f query='
query($owner:String!,$name:String!,$endCursor:String){repository(owner:$owner,name:$name){
issues(first:100,after:$endCursor){pageInfo{hasNextPage endCursor}nodes{number parent{number} blockedBy(first:50){nodes{number state}}}}}}' > "$tmp/relations.json"
"$here/gh-to-beads.sh" "$repo" "$tmp/relations.json" < "$tmp/issues.json" > "$tmp/beads.json"
mv "$tmp/beads.json" "${WHEEL_DATA_DIR:-.}/beads.json"
