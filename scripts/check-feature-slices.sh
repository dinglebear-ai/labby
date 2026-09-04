#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

run cargo check -p labby-apis --no-default-features
run cargo check -p labby-apis --no-default-features --features all

run cargo check -p labby-auth --no-default-features --all-targets
run cargo check -p labby-auth --no-default-features --features http-axum --all-targets
run cargo check -p labby-auth --no-default-features --features upstream-oauth-rmcp --all-targets
run cargo check -p labby-auth --no-default-features --features http-axum,upstream-oauth-rmcp --all-targets

run cargo check -p labby-codemode --all-targets
run cargo check -p labby-gateway --all-targets
run cargo check -p labby-web --all-targets
run cargo check -p labby-winjob --all-targets

run cargo check -p labby-runtime --no-default-features --all-targets

labby_product_features=(
  ""
  "gateway"
  "gateway-host"
  "integrated-gateway"
  "fs"
  "skills"
  "all"
)

for features in "${labby_product_features[@]}"; do
  if [[ -z "$features" ]]; then
    run cargo check -p labby --no-default-features --all-targets
  else
    run cargo check -p labby --no-default-features --features "$features" --all-targets
  fi
done
run cargo check -p labby --all-features --all-targets
