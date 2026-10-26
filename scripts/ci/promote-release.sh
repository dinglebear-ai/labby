#!/usr/bin/env bash
set -euo pipefail
tag=${RELEASE_TAG:?RELEASE_TAG is required}
channel=$(python3 "$(dirname "$0")/release_channel.py" "$tag" | python3 -c 'import json,sys; print(json.load(sys.stdin)["channel"])')
gh release view "$tag" --json isDraft --jq .isDraft | grep -Fx true >/dev/null
if [[ "$channel" == dev ]]; then
  gh release edit "$tag" --draft=false --prerelease --latest=false
else
  gh release edit "$tag" --draft=false
fi
for attempt in 1 2 3 4 5; do
  state=$(gh release view "$tag" --json isDraft --jq .isDraft)
  if [[ "$state" == false ]]; then
    if [[ "$channel" == dev ]]; then
      gh release view "$tag" --json isPrerelease --jq .isPrerelease | grep -Fx true >/dev/null
      latest=$(gh api "repos/${GITHUB_REPOSITORY:?}/releases/latest" --jq .tag_name)
      [[ "$latest" != "$tag" ]] || { echo 'dev release became GitHub Latest' >&2; exit 1; }
    fi
    exit 0
  fi
  ((attempt == 5)) || sleep "${PROMOTION_VERIFY_DELAY_SECONDS:-2}"
done
echo "release $tag did not become publicly visible" >&2
exit 1
