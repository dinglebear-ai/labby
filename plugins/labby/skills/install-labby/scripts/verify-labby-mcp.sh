#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: verify-labby-mcp.sh <mcp-url>

Perform a read-only MCP smoke test against a running Labby HTTP endpoint.

Environment:
  LABBY_MCP_TOKEN             Bearer token (required for bearer-authenticated endpoints)
  LABBY_MCP_PROTOCOL_VERSION  MCP protocol version (default: 2025-06-18)
  LABBY_EXPECTED_VERSION      Optional exact version expected from `labby --version`
  LABBY_BIN                   Labby executable to inspect (default: labby)
  LABBY_MCP_CONNECT_TIMEOUT   Connection timeout in seconds (default: 10)
  LABBY_MCP_MAX_TIME          Total request timeout in seconds (default: 30)
EOF
}

if [[ ${1:-} == "--help" || ${1:-} == "-h" ]]; then
  usage
  exit 0
fi

if [[ $# -ne 1 ]]; then
  usage >&2
  exit 64
fi

for command_name in curl jq; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "required command not found: ${command_name}" >&2
    exit 69
  fi
done

mcp_url=$1
protocol_version=${LABBY_MCP_PROTOCOL_VERSION:-2025-06-18}
labby_bin=${LABBY_BIN:-labby}
connect_timeout=${LABBY_MCP_CONNECT_TIMEOUT:-10}
max_time=${LABBY_MCP_MAX_TIME:-30}

if [[ "$mcp_url" != http://* && "$mcp_url" != https://* ]]; then
  echo "MCP URL must use http:// or https://" >&2
  exit 64
fi

if [[ "$protocol_version" == *$'\n'* || "$protocol_version" == *$'\r'* ]]; then
  echo "MCP protocol version must be a single line" >&2
  exit 64
fi

[[ "$connect_timeout" =~ ^[1-9][0-9]*$ ]] || { echo "LABBY_MCP_CONNECT_TIMEOUT must be a positive integer" >&2; exit 64; }
[[ "$max_time" =~ ^[1-9][0-9]*$ ]] || { echo "LABBY_MCP_MAX_TIME must be a positive integer" >&2; exit 64; }

if [[ ${LABBY_MCP_TOKEN:-} == *$'\n'* || ${LABBY_MCP_TOKEN:-} == *$'\r'* ]]; then
  echo "LABBY_MCP_TOKEN must be a single line" >&2
  exit 64
fi

if [[ -n ${LABBY_EXPECTED_VERSION:-} ]]; then
  if ! command -v "$labby_bin" >/dev/null 2>&1; then
    echo "Labby executable not found for version verification: ${labby_bin}" >&2
    exit 69
  fi

  installed_version="$($labby_bin --version)"
  if [[ "$installed_version" != "labby ${LABBY_EXPECTED_VERSION}" ]]; then
    echo "Labby version mismatch: expected labby ${LABBY_EXPECTED_VERSION}, got ${installed_version}" >&2
    exit 65
  fi
fi

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/verify-labby-mcp.XXXXXX")
trap 'rm -rf -- "$work_dir"' EXIT
chmod 700 "$work_dir"

curl_config="$work_dir/curl.conf"
response_file="$work_dir/response.json"
{
  printf 'silent\nshow-error\n'
  printf 'header = "Content-Type: application/json"\n'
  printf 'header = "Accept: application/json, text/event-stream"\n'
  printf 'header = "MCP-Protocol-Version: %s"\n' "$protocol_version"
  if [[ -n ${LABBY_MCP_TOKEN:-} ]]; then
    escaped_token=${LABBY_MCP_TOKEN//\\/\\\\}
    escaped_token=${escaped_token//\"/\\\"}
    printf 'header = "Authorization: Bearer %s"\n' "$escaped_token"
  fi
} >"$curl_config"
chmod 600 "$curl_config"

mcp_request() {
  local method=$1
  local payload=$2
  local status

  status=$(curl --config "$curl_config" \
    --connect-timeout "$connect_timeout" \
    --max-time "$max_time" \
    --output "$response_file" \
    --write-out '%{http_code}' \
    --header "Mcp-Method: ${method}" \
    --data "$payload" \
    "$mcp_url")

  if [[ "$status" != 200 ]]; then
    echo "Labby MCP ${method} returned HTTP ${status}" >&2
    jq -c '{error: .error}' "$response_file" >&2 2>/dev/null || true
    return 1
  fi

  if ! jq -e '.jsonrpc == "2.0" and (.error == null)' "$response_file" >/dev/null; then
    echo "Labby MCP ${method} returned an invalid or error response" >&2
    jq -c '{jsonrpc, id, error}' "$response_file" >&2 2>/dev/null || true
    return 1
  fi
}

list_request=$(jq -cn \
  --arg version "$protocol_version" \
  '{jsonrpc:"2.0",id:1,method:"tools/list",params:{_meta:{"io.modelcontextprotocol/protocolVersion":$version,"io.modelcontextprotocol/clientInfo":{name:"install-labby-verifier",version:"1"},"io.modelcontextprotocol/clientCapabilities":{}}}}')
mcp_request tools/list "$list_request"

if ! jq -e '.result.tools | map(.name) | index("gateway") != null' "$response_file" >/dev/null; then
  echo "Labby MCP tools/list did not expose the gateway tool" >&2
  exit 1
fi

tool_count=$(jq '.result.tools | length' "$response_file")

help_request='{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"gateway","arguments":{"action":"help","params":{}}}}'
mcp_request tools/call "$help_request"

if ! jq -e '.result.isError != true and (.result.content | type == "array" and length > 0)' "$response_file" >/dev/null; then
  echo "Labby gateway.help did not return successful MCP content" >&2
  exit 1
fi

printf 'Labby MCP verification passed: protocol=%s tools=%s gateway.help=ok\n' \
  "$protocol_version" "$tool_count"
