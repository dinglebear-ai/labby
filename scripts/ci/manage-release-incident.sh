#!/usr/bin/env bash
set -euo pipefail

complete=${1:?true or false required}
case "$complete" in
  true|false) ;;
  *) printf 'expected true or false, got %s\n' "$complete" >&2; exit 2 ;;
esac

title='Release publication is incomplete'
issue_numbers=$(gh api --paginate "repos/$GITHUB_REPOSITORY/issues?state=open&per_page=100" \
  --jq '.[] | select((has("pull_request") | not) and .title == "Release publication is incomplete") | .number')
number=${issue_numbers%%$'\n'*}

if [[ "$complete" == true ]]; then
  if [[ -n "$number" ]]; then
    gh api --method POST "repos/$GITHUB_REPOSITORY/issues/$number/comments" \
      -f body='All release distributions now match the immutable manifest.' >/dev/null
    gh api --method PATCH "repos/$GITHUB_REPOSITORY/issues/$number" \
      -f state=closed >/dev/null
  fi
  exit 0
fi

reconciliation=$(<reconciliation.json)
body=$(printf 'The aggregate release reconciler found a mismatch.\n\n```json\n%s\n```\n\nRun: %s' \
  "$reconciliation" "$GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID")
if [[ -n "$number" ]]; then
  gh api --method PATCH "repos/$GITHUB_REPOSITORY/issues/$number" \
    -f body="$body" >/dev/null
else
  gh api --method POST "repos/$GITHUB_REPOSITORY/issues" \
    -f title="$title" -f body="$body" >/dev/null
fi
