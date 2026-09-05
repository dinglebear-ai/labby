#!/usr/bin/env bash
set -euo pipefail

tag=${RELEASE_TAG:?RELEASE_TAG is required}
[[ ${1:-} == -- ]] && shift
(($# > 0)) || { echo 'at least one release asset is required' >&2; exit 64; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
for asset in "$@"; do
  [[ -f $asset ]] || { echo "missing release asset: $asset" >&2; exit 1; }
  name=${asset##*/}
  if gh release download "$tag" --pattern "$name" --dir "$tmp" >/dev/null 2>&1; then
    cmp -s "$asset" "$tmp/$name" || {
      echo "immutable release asset differs from existing bytes: $name" >&2
      exit 73
    }
  else
    gh release upload "$tag" "$asset"
  fi
done
