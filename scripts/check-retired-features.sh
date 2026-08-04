#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

failed=0

forbidden_paths=(
  crates/labby-apis/src/acp.rs
  crates/labby-apis/src/acp
  crates/labby-apis/src/acp_registry.rs
  crates/labby-apis/src/acp_registry
  crates/labby-apis/src/mcpregistry.rs
  crates/labby-apis/src/mcpregistry
  crates/labby-apis/src/marketplace.rs
  crates/labby-apis/src/marketplace
  crates/labby-apis/src/device_runtime.rs
  crates/labby-apis/src/device_runtime
  crates/labby-apis/src/deploy.rs
  crates/labby-apis/src/deploy
  crates/labby-apis/src/stash.rs
  crates/labby-apis/src/stash
  apps/gateway-admin/components/registry
  apps/gateway-admin/lib/api/mcpregistry-client.ts
  apps/gateway-admin/lib/hooks/use-registry.ts
  config/acp-adapters.package.json
  config/acp-providers.docker.json
  plugins/scripts/acp-smoke-check
)

for path in "${forbidden_paths[@]}"; do
  if [[ -e "$path" ]]; then
    printf 'retired-feature guard: forbidden path exists: %s
' "$path" >&2
    failed=1
  fi
done

active_roots=(
  Cargo.toml
  crates
  apps/gateway-admin/app
  apps/gateway-admin/components
  apps/gateway-admin/lib
  config
  scripts
  plugins/scripts
  .github
  docker-compose.yml
  docker-compose.prod.yml
)

forbidden_pattern='pub mod (acp|acp_registry|mcpregistry|marketplace|device_runtime|deploy|stash)|feature = "(acp_registry|mcpregistry|marketplace|deploy)"|labby_apis::(acp|acp_registry|mcpregistry|marketplace|device_runtime|deploy|stash)|mcpregistry.url|ACP_SESSION_CWD|NodeRuntimeRole|DevicePreferences|ResolvedDeviceRuntime|/v1/(acp|stash|marketplace|nodes|fleet)|/dev/api/marketplace|marketplaceActionUrl|nodeDetailUrl|nodeLogsSearchUrl'

# Portable POSIX grep, not ripgrep: this guard runs in the lightweight `changes`
# CI job, which installs no extra tooling. A missing `rg` previously made the
# scan silently pass while flipping the two presence checks below into false
# failures.
scan_roots=()
for root in "${active_roots[@]}"; do
  [[ -e "$root" ]] && scan_roots+=("$root")
done

if (( ${#scan_roots[@]} == 0 )); then
  printf 'retired-feature guard: no scannable roots found; refusing to pass vacuously\n' >&2
  exit 1
fi

# grep exits 0 on match, 1 on no match, >1 on error. Only 1 is a clean pass.
set +e
grep -rEn --binary-files=without-match \
  --exclude-dir=.git \
  --exclude-dir=target \
  --exclude-dir=node_modules \
  --exclude='check-retired-features.sh' \
  -- "$forbidden_pattern" "${scan_roots[@]}"
grep_status=$?
set -e

case "$grep_status" in
  0)
    printf 'retired-feature guard: forbidden active identifier found\n' >&2
    failed=1
    ;;
  1) ;;
  *)
    printf 'retired-feature guard: scan failed (grep exit %s)\n' "$grep_status" >&2
    failed=1
    ;;
esac

require_present() {
  local pattern="$1" file="$2" message="$3"
  if [[ ! -f "$file" ]]; then
    printf 'retired-feature guard: %s (missing file: %s)\n' "$message" "$file" >&2
    failed=1
    return
  fi
  if ! grep -qE -- "$pattern" "$file"; then
    printf 'retired-feature guard: %s\n' "$message" >&2
    failed=1
  fi
}

require_absent() {
  local pattern="$1" file="$2" message="$3"
  if [[ -f "$file" ]] && grep -qE -- "$pattern" "$file"; then
    printf 'retired-feature guard: %s\n' "$message" >&2
    failed=1
  fi
}

require_present 'io\.modelcontextprotocol\.registry/publisher-provided' server.json \
  'server.json no longer publishes Labby to the official MCP Registry'
require_present 'dinglebear-ai/workflows/\.github/workflows/mcp-registry-publish\.yml@3302f853574ba0c669a647f66cfcacb81f529fff' .github/workflows/mcp-registry.yml 'MCP Registry publication must use the canonical pinned shared workflow'
require_present 'auth-method:[[:space:]]+dns' .github/workflows/mcp-registry.yml 'MCP Registry publication must use DNS ownership authentication'
require_present 'MCP_PRIVATE_KEY:.*secrets\.MCP_PRIVATE_KEY' .github/workflows/mcp-registry.yml 'MCP Registry publication must pass the DNS signing key to the shared workflow'
require_absent 'mcp-publisher|registry\.modelcontextprotocol\.io|MCP_REGISTRY_DOMAIN' .github/workflows/release.yml 'release.yml must not duplicate the shared MCP Registry publisher'
require_absent 'tootie\.tv' .github/workflows/mcp-registry.yml 'MCP Registry publication must not use the homelab tootie.tv domain'

if (( failed != 0 )); then
  exit 1
fi

printf 'retired-feature guard passed
'
