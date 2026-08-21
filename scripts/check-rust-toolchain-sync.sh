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

rust_toolchain_action_ref="dtolnay/rust-toolchain@4cda84d5c5c54efe2404f9d843567869ab1699d4"

require_text .github/actions/setup-rust-kache/action.yml "uses: $rust_toolchain_action_ref"
require_text .github/workflows/ci.yml "uses: $rust_toolchain_action_ref"
require_text .github/workflows/release.yml "uses: $rust_toolchain_action_ref"
require_text .github/workflows/release.yml "toolchain: \"$expected\""

check_direct_rust_action_toolchains() {
  local file=$1
  if ! awk -v ref="$rust_toolchain_action_ref" -v expected="$expected" '
    index($0, "uses: " ref) {
      if (pending) bad=1
      pending=1
      next
    }
    pending && index($0, "toolchain: \"" expected "\"") {
      pending=0
      next
    }
    pending && $0 ~ /^[[:space:]]*-[[:space:]]/ {
      bad=1
      pending=0
    }
    END { exit((bad || pending) ? 1 : 0) }
  ' "$file"; then
    echo "[rust-toolchain-sync] FAIL — $file has a direct Rust action without explicit toolchain \"$expected\"" >&2
    exit 1
  fi
}

check_direct_rust_action_toolchains .github/workflows/ci.yml
check_direct_rust_action_toolchains .github/workflows/release.yml

unexpected_action_refs=$(
  grep -RhEo 'dtolnay/rust-toolchain@[0-9a-f]{40}' .github/actions .github/workflows \
    | sort -u \
    | grep -Fvx -- "$rust_toolchain_action_ref" \
    || true
)
if [[ -n "$unexpected_action_refs" ]]; then
  echo "[rust-toolchain-sync] FAIL — unexpected dtolnay/rust-toolchain pin(s):" >&2
  printf '%s\n' "$unexpected_action_refs" >&2
  exit 1
fi

echo "[rust-toolchain-sync] OK — active Rust/MSRV contracts use $expected"
