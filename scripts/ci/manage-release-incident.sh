#!/usr/bin/env bash
set -euo pipefail
complete=${1:?true or false required}
title='Release publication is incomplete'
number=$(gh issue list --state open --search "$title in:title" --json number,title --jq ".[] | select(.title == \"$title\") | .number" | head -1)
if [[ "$complete" == true ]]; then
  [[ -z "$number" ]] || gh issue close "$number" --comment 'All release distributions now match the immutable manifest.'
  exit 0
fi
body=$(printf 'The aggregate release reconciler found a mismatch.\n\n```json\n%s\n```\n\nRun: %s' "$(cat reconciliation.json)" "$GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID")
if [[ -n "$number" ]]; then
  gh issue edit "$number" --body "$body"
else
  gh issue create --title "$title" --body "$body"
fi
