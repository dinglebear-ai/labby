#!/usr/bin/env bash
set -euo pipefail

image=${LABBY_IMAGE:-}
if [[ ! "$image" =~ ^ghcr\.io/dinglebear-ai/labby@sha256:[0-9a-f]{64}$ ]]; then
  echo "LABBY_IMAGE must be the canonical immutable GHCR image pinned by a 64-hex sha256 digest" >&2
  exit 64
fi

for variable in LABBY_BUILDER_IMAGE LABBY_RUNTIME_IMAGE; do
  value=${!variable:-}
  if [[ ! "$value" =~ ^[^[:space:]@]+@sha256:[0-9a-f]{64}$ ]]; then
    echo "$variable must be an image pinned by a 64-hex sha256 digest" >&2
    exit 64
  fi
done
