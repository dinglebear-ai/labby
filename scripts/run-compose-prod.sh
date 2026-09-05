#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$repo_root/config/container-supply.conf"
export LABBY_BUILDER_IMAGE LABBY_RUNTIME_IMAGE
"$repo_root/scripts/ci/validate-container-inputs.sh"
: "${LABBY_RELEASE_TAG:?set LABBY_RELEASE_TAG to the attested vMAJOR.MINOR.PATCH release tag}"
[[ "$LABBY_RELEASE_TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "LABBY_RELEASE_TAG must be a stable vMAJOR.MINOR.PATCH tag" >&2; exit 64; }
command -v gh >/dev/null 2>&1 || { echo "gh is required to verify image provenance" >&2; exit 1; }
gh attestation verify "oci://$LABBY_IMAGE" \
  --repo dinglebear-ai/labby \
  --signer-workflow dinglebear-ai/labby/.github/workflows/release.yml \
  --source-ref "refs/tags/$LABBY_RELEASE_TAG" \
  --deny-self-hosted-runners >/dev/null
exec docker compose -f "$repo_root/docker-compose.prod.yml" "$@"
