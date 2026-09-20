#!/usr/bin/env bash
set -euo pipefail

complete=${1:?true or false required}
case "$complete" in
  true|false) ;;
  *)
    printf 'expected true or false, got %q\n' "$complete" >&2
    exit 2
    ;;
esac

title='Release publication is incomplete'
issues_json=$(gh api --paginate "repos/$GITHUB_REPOSITORY/issues?state=open&per_page=100")
number=$(jq -sr --arg title "$title" '
  [.[][] | select((has("pull_request") | not) and .title == $title) | .number][0] // empty
' <<<"$issues_json")

if [[ "$complete" == true ]]; then
  if [[ -n "$number" ]]; then
    gh api --method POST "repos/$GITHUB_REPOSITORY/issues/$number/comments" \
      -f body='All release distributions now match the immutable manifest.' >/dev/null
    gh api --method PATCH "repos/$GITHUB_REPOSITORY/issues/$number" \
      -f state=closed >/dev/null
  fi
  exit 0
fi

body=$(printf 'The aggregate release reconciler found a mismatch.\n\n```json\n%s\n```\n\nRun: %s' "$(cat reconciliation.json)" "$GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID")
if [[ -n "$number" ]]; then
  gh api --method PATCH "repos/$GITHUB_REPOSITORY/issues/$number" \
    -f body="$body" >/dev/null
else
  gh api --method POST "repos/$GITHUB_REPOSITORY/issues" \
    -f title="$title" \
    -f body="$body" >/dev/null
fi
