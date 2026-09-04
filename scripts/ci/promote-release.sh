#!/usr/bin/env bash
set -euo pipefail
tag=${RELEASE_TAG:?RELEASE_TAG is required}
gh release view "$tag" --json isDraft --jq .isDraft | grep -Fx true >/dev/null
gh release edit "$tag" --draft=false
for attempt in 1 2 3 4 5; do
  state=$(gh release view "$tag" --json isDraft --jq .isDraft)
  [[ "$state" == false ]] && exit 0
  ((attempt == 5)) || sleep "${PROMOTION_VERIFY_DELAY_SECONDS:-2}"
done
echo "release $tag did not become publicly visible" >&2
exit 1
