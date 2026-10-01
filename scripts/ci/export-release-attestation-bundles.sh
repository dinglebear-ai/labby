#!/usr/bin/env bash
set -euo pipefail

# Publish untrusted signed bundles alongside immutable release artifacts.
# Verification authenticates them against gh's maintained Sigstore roots.
repo=${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}
release_tag=${RELEASE_TAG:?RELEASE_TAG is required}
(($#)) || { echo "usage: export-release-attestation-bundles.sh <artifact>..." >&2; exit 64; }
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=scripts/ci/github-verifier-bootstrap.sh
source "$script_dir/github-verifier-bootstrap.sh"
verifier_dir=$(mktemp -d)
bundle_dir=
trap 'rm -rf "$verifier_dir" "${bundle_dir:-}"' EXIT
ensure_github_verifier "$verifier_dir"
for artifact in "$@"; do
  [[ -f "$artifact" && ! -L "$artifact" ]] || { echo "release artifact must be a regular file" >&2; exit 1; }
  artifact_path=$(cd "$(dirname "$artifact")" && pwd)/$(basename "$artifact")
  bundle_dir=$(mktemp -d)
  (cd "$bundle_dir" && "$GH_VERIFIER" attestation download "$artifact_path" --hostname github.com --repo "$repo")
  shopt -s nullglob
  bundles=("$bundle_dir"/*.jsonl)
  ((${#bundles[@]} == 1)) || { echo "expected one digest-bound attestation bundle" >&2; exit 1; }
  # An empty credential store proves this public-bundle path needs no account.
  mkdir "$bundle_dir/consumer-config"
  GH_TOKEN='' GITHUB_TOKEN='' GH_ENTERPRISE_TOKEN='' GITHUB_ENTERPRISE_TOKEN='' GH_HOST=github.com GH_CONFIG_DIR="$bundle_dir/consumer-config" GH_VERIFIER="$GH_VERIFIER" \
    "$script_dir/verify-release-provenance.sh" --repo "$repo" --workflow release.yml \
      --ref "refs/tags/$release_tag" --artifact "$artifact_path" --bundle "${bundles[0]}"
  cp "${bundles[0]}" "$artifact_path.sigstore.jsonl"
  rm -rf "$bundle_dir"
  bundle_dir=
done
