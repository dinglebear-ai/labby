#!/usr/bin/env bash
set -euo pipefail

args=()
while (($#)); do
  [[ "$1" == -- ]] && { shift; break; }
  args+=("$1")
  shift
done
(($#)) || { echo 'activation command is required after --' >&2; exit 64; }
scripts/ci/verify-release-provenance.sh "${args[@]}"
exec "$@"
