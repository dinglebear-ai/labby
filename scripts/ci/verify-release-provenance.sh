#!/usr/bin/env bash
set -euo pipefail

repo=""; workflow=""; ref=""; bundle=""; trusted_root=""; artifact=""
while (($#)); do
  case "$1" in
    --repo) repo=$2; shift 2 ;;
    --workflow) workflow=$2; shift 2 ;;
    --ref) ref=$2; shift 2 ;;
    --bundle) bundle=$2; shift 2 ;;
    --trusted-root) trusted_root=$2; shift 2 ;;
    --artifact) artifact=$2; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done
[[ -n "$repo" && -n "$workflow" && -n "$ref" && -n "$artifact" ]] || exit 64
args=(attestation verify "$artifact" --repo "$repo" --signer-workflow "$repo/.github/workflows/$workflow" --source-ref "$ref" --deny-self-hosted-runners)
[[ -z "$bundle" ]] || args+=(--bundle "$bundle")
[[ -z "$trusted_root" ]] || args+=(--custom-trusted-root "$trusted_root")
gh "${args[@]}" >/dev/null
