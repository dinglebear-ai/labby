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

if rg -n --hidden \
  --glob '!.git/**' \
  --glob '!target/**' \
  --glob '!scripts/check-retired-features.sh' \
  "$forbidden_pattern" "${active_roots[@]}"; then
  printf 'retired-feature guard: forbidden active identifier found
' >&2
  failed=1
fi

if ! rg -q 'io\.modelcontextprotocol\.registry/publisher-provided' server.json; then
  printf 'retired-feature guard: server.json no longer publishes Labby to the official MCP Registry
' >&2
  failed=1
fi

if ! rg -q 'mcp-publisher' .github/workflows/release.yml; then
  printf 'retired-feature guard: MCP Registry publication workflow is missing
' >&2
  failed=1
fi

if (( failed != 0 )); then
  exit 1
fi

printf 'retired-feature guard passed
'
