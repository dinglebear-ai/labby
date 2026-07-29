#!/usr/bin/env bash
# Verify the active Rust/MSRV contracts agree with Cargo.toml.
set -euo pipefail

project_dir="${1:-.}"
cd "$project_dir"

expected=$(
  sed -n '/^\[workspace\.package\]/,/^\[/s/^rust-version = "\([^"]*\)"/\1/p' Cargo.toml \
    | head -n 1
)
toolchain=$(
  sed -n '/^\[toolchain\]/,/^\[/s/^channel = "\([^"]*\)"/\1/p' rust-toolchain.toml \
    | head -n 1
)

if [[ -z "$expected" || -z "$toolchain" ]]; then
  echo "[rust-toolchain-sync] FAIL — could not read Cargo or rust-toolchain pin" >&2
  exit 1
fi

if [[ "$toolchain" != "$expected" ]]; then
  echo "[rust-toolchain-sync] FAIL — Cargo MSRV $expected != toolchain $toolchain" >&2
  exit 1
fi

require_text() {
  local file=$1
  local text=$2
  if ! grep -Fq -- "$text" "$file"; then
    echo "[rust-toolchain-sync] FAIL — $file is missing: $text" >&2
    exit 1
  fi
}

require_text README.md "Rust $expected or newer."
require_text packages/labby-mcp/README.md "Rust $expected or newer."
require_text CLAUDE.md "msrv\` ($expected)"
require_text .github/CLAUDE.md "cargo +$expected check --workspace --all-features --all-targets --locked"
require_text docs/runtime/CICD.md "cargo +$expected check --workspace --all-features --all-targets --locked"
require_text config/Dockerfile "Requires Rust $expected+ (Cargo.toml: rust-version = \"$expected\""
require_text .github/actions/setup-rust-kache/action.yml "default: \"$expected\""
require_text .github/workflows/ci.yml "toolchain: \"$expected\""
require_text .github/workflows/ci.yml "cargo +$expected check --workspace --all-features --all-targets --locked"

echo "[rust-toolchain-sync] OK — active Rust/MSRV contracts use $expected"
