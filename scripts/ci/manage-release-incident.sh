#!/usr/bin/env bash
set -euo pipefail
complete=${1:?true or false required}
title='Release publication is incomplete'
issues_endpoint="repos/$GITHUB_REPOSITORY/issues"

open_issues=$(gh api --paginate --slurp "$issues_endpoint?state=open&per_page=100")
number=$(
  python3 -c '
import json
import sys

pages = json.load(sys.stdin)
title = sys.argv[1]
for page in pages:
    for issue in page:
        if "pull_request" not in issue and issue.get("title") == title:
            print(issue["number"])
            raise SystemExit(0)
' "$title" <<<"$open_issues"
)

if [[ "$complete" == true ]]; then
  if [[ -n "$number" ]]; then
    gh api --method POST "$issues_endpoint/$number/comments" \
      -f body='All release distributions now match the immutable manifest.' >/dev/null
    gh api --method PATCH "$issues_endpoint/$number" \
      -f state=closed -f state_reason=completed >/dev/null
  fi
  exit 0
fi

body=$(printf 'The aggregate release reconciler found a mismatch.\n\n```json\n%s\n```\n\nRun: %s' "$(cat reconciliation.json)" "$GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID")
if [[ -n "$number" ]]; then
  gh api --method PATCH "$issues_endpoint/$number" -f body="$body" >/dev/null
else
  gh api --method POST "$issues_endpoint" -f title="$title" -f body="$body" >/dev/null
fi
