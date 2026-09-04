#!/usr/bin/env bash
set -euo pipefail

# Verify Labby's production rmcp pin, then run the matching upstream
# rmcp 3.1.0 fixture against the 2026-07-28 dated protocol, Labby-native
# multi-hop proxying, the direct stdio proxy probe, and the separately scored
# extension suite.
#
# JavaScript dependencies are installed exactly once before any scenario runs.
# This avoids concurrent npx cache mutation when scenarios are executed in CI.

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  cat <<'EOF'
Usage: scripts/ci/mcp-conformance.sh [--direct-proxy-only]

Runs the pinned MCP conformance suite and a direct stdio proxy probe.
--direct-proxy-only runs only the real Labby + fixture stdio server scenario.
Set MCP_CONFORMANCE_OUTPUT_DIR to choose the artifact directory.
EOF
  exit 0
fi

direct_proxy_only=false
if [[ "${1:-}" == "--direct-proxy-only" ]]; then
  direct_proxy_only=true
  shift
fi
if [[ $# -ne 0 ]]; then
  echo "unknown argument: $1" >&2
  exit 2
fi

LABBY_RMCP_REPOSITORY="${LABBY_RMCP_REPOSITORY:-https://github.com/dinglebear-ai/rust-sdk.git}"
LABBY_RMCP_REVISION="${LABBY_RMCP_REVISION:-0665dcac527abd6828a6bdc805821e820841e491}"
RMCP_FIXTURE_VERSION="${RMCP_FIXTURE_VERSION:-3.1.0}"
RMCP_TAG="${RMCP_TAG:-rmcp-v${RMCP_FIXTURE_VERSION}}"
RMCP_COMMIT="${RMCP_COMMIT:-1f9358eddca42d3a510c70ae6446dd6548c7c856}"
MCP_CONFORMANCE_VERSION="${MCP_CONFORMANCE_VERSION:-0.2.0-alpha.10}"
MCP_SPEC_VERSION="${MCP_SPEC_VERSION:-2026-07-28}"
MCP_CONFORMANCE_PORT="${MCP_CONFORMANCE_PORT:-18002}"
MCP_CONFORMANCE_LABBY_PORT="${MCP_CONFORMANCE_LABBY_PORT:-18003}"
MCP_CONFORMANCE_DIRECT_PROXY_PORT="${MCP_CONFORMANCE_DIRECT_PROXY_PORT:-18004}"
MCP_CONFORMANCE_OUTPUT_DIR="${MCP_CONFORMANCE_OUTPUT_DIR:-target/mcp-conformance}"

repo_root="$(git rev-parse --show-toplevel)"
if [[ "$MCP_CONFORMANCE_OUTPUT_DIR" = /* ]]; then
  output_dir="$MCP_CONFORMANCE_OUTPUT_DIR"
else
  output_dir="${repo_root}/${MCP_CONFORMANCE_OUTPUT_DIR}"
fi
cargo_target_dir="${CARGO_TARGET_DIR:-${repo_root}/target}"
dated_baseline="${repo_root}/conformance/expected-failures-dated.yaml"
extension_baseline="${repo_root}/conformance/expected-failures-extensions.yaml"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/labby-mcp-conformance.XXXXXX")"
rmcp_target_dir="${CARGO_TARGET_DIR:-${work_dir}/rust-sdk/target}"
server_pid=""
labby_pid=""
direct_proxy_pid=""

cleanup() {
  if [[ -n "$direct_proxy_pid" ]]; then
    kill -INT "$direct_proxy_pid" 2>/dev/null || true
    wait "$direct_proxy_pid" 2>/dev/null || true
  fi
  if [[ -n "$server_pid" ]]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  if [[ -n "$labby_pid" ]]; then
    kill "$labby_pid" 2>/dev/null || true
    wait "$labby_pid" 2>/dev/null || true
  fi
  rm -rf "$work_dir"
}
trap cleanup EXIT

if ! grep -Fq \
  "rmcp = { git = \"${LABBY_RMCP_REPOSITORY}\", rev = \"${LABBY_RMCP_REVISION}\"" \
  "${repo_root}/Cargo.toml"; then
  echo \
    "Cargo.toml must pin rmcp to ${LABBY_RMCP_REPOSITORY}@${LABBY_RMCP_REVISION}" \
    >&2
  exit 1
fi

mkdir -p "$output_dir"

run_direct_proxy() {
  local ready_file="${work_dir}/direct-proxy-ready.json"
  local error_file="${work_dir}/direct-proxy.stderr"
  local child_pid_file="${work_dir}/direct-proxy-child.pid"
  local direct_home="${work_dir}/direct-home"
  mkdir -p "$direct_home"

  # Identical to the top-level build below so the second invocation is a
  # no-op rather than a third distinct fingerprint.
  cargo build -p labby --all-features --features proxy-testkit --locked \
    --bins \
    --example mcp_multihop_conformance
  HOME="$direct_home" LABBY_HOME="$direct_home" LABBY_LOG="labby=warn" \
    "${cargo_target_dir}/debug/labby" --json proxy --local --auth none \
    --port "$MCP_CONFORMANCE_DIRECT_PROXY_PORT" \
    "${cargo_target_dir}/debug/stdio-mcp-fixture" --pid-file "$child_pid_file" \
    >"$ready_file" 2>"$error_file" &
  direct_proxy_pid="$!"

  local ready=false
  for _ in $(seq 1 100); do
    if [[ -s "$ready_file" ]] && jq --exit-status \
      '.url and (.local_addr | startswith("127.0.0.1:"))' "$ready_file" >/dev/null 2>&1; then
      ready=true
      break
    fi
    if ! kill -0 "$direct_proxy_pid" 2>/dev/null; then
      break
    fi
    sleep 0.1
  done
  if [[ "$ready" != true ]]; then
    echo "direct stdio proxy did not become ready" >&2
    return 1
  fi

local direct_url
direct_url="$(jq -r .url "$ready_file")"
  local method marker body_file status
  for method_marker in \
    'tools/list|fixture.echo' \
    'resources/list|fixture://status' \
    'prompts/list|fixture.prompt'; do
    IFS='|' read -r method marker <<<"$method_marker"
    body_file="${work_dir}/direct-${method//\//-}.body"
    status="$(curl --silent --show-error --output "$body_file" --write-out '%{http_code}' \
      --header 'Content-Type: application/json' \
      --header 'Accept: application/json, text/event-stream' \
      --header 'MCP-Protocol-Version: 2026-07-28' \
      --header "Mcp-Method: ${method}" \
      --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":{\"_meta\":{\"io.modelcontextprotocol/protocolVersion\":\"2026-07-28\",\"io.modelcontextprotocol/clientInfo\":{\"name\":\"direct-proxy-conformance\",\"version\":\"1\"},\"io.modelcontextprotocol/clientCapabilities\":{}}}}" \
      "$direct_url")"
    if [[ "$status" != 200 ]] || ! grep -Fq "$marker" "$body_file"; then
      echo "direct proxy ${method} failed with HTTP ${status}" >&2
      return 1
    fi
  done

  kill -INT "$direct_proxy_pid"
  wait "$direct_proxy_pid"
  direct_proxy_pid=""
  if [[ -f "$child_pid_file" ]] && kill -0 "$(<"$child_pid_file")" 2>/dev/null; then
    echo "direct proxy fixture child survived Ctrl+C" >&2
    return 1
  fi

  cat >"${output_dir}/direct-proxy.json" <<'EOF'
{
  "auth": "none",
  "bind": "loopback",
  "cleanup": "passed",
  "fixture": "stdio-mcp-fixture",
  "primitives": ["prompts", "resources", "tools"],
  "result": "passed",
  "runtime": "labby"
}
EOF
}

if [[ "$direct_proxy_only" == true ]]; then
  run_direct_proxy
  echo "Direct proxy conformance results written to ${output_dir}"
  exit 0
fi

git clone --quiet --depth 1 --branch "$RMCP_TAG" \
  https://github.com/modelcontextprotocol/rust-sdk.git \
  "${work_dir}/rust-sdk"

actual_commit="$(git -C "${work_dir}/rust-sdk" rev-parse HEAD)"
if [[ "$actual_commit" != "$RMCP_COMMIT" ]]; then
  echo "rmcp tag ${RMCP_TAG} resolved to ${actual_commit}, expected ${RMCP_COMMIT}" >&2
  exit 1
fi

npm_config_cache="${work_dir}/npm-cache" npm install \
  --prefix "${work_dir}/js" \
  --ignore-scripts \
  --no-audit \
  --no-fund \
  "@modelcontextprotocol/conformance@${MCP_CONFORMANCE_VERSION}"

conformance="${work_dir}/js/node_modules/.bin/conformance"

cargo build \
  --manifest-path "${work_dir}/rust-sdk/Cargo.toml" \
  -p mcp-conformance
cargo test \
  --manifest-path "${work_dir}/rust-sdk/Cargo.toml" \
  -p mcp-conformance \
  --bin conformance-server
cargo build -p labby --all-features --features proxy-testkit --locked \
  --bins \
  --example mcp_multihop_conformance

# Exercise a real client -> root Labby -> middle Labby -> leaf chain. The
# driver verifies modern discovery; multi-page tools, prompts, resources, and
# templates; tool and MRTR forwarding; task lifecycle; progress and cancellation;
# mutable subscription catalogs; resource reads; completion; and provenance.
"${cargo_target_dir}/debug/examples/mcp_multihop_conformance" driver \
  >"${output_dir}/labby-multihop.log" 2>&1
grep --fixed-strings --line-regexp "Labby multi-hop conformance passed" \
  "${output_dir}/labby-multihop.log" >/dev/null

# Exercise Labby's real authenticated, stateless HTTP boundary with production
# tools. The upstream conformance fixture remains necessary because the dated
# suite calls synthetic image/audio/resource/prompt tools that must not be
# exposed by a production Labby catalog.
conformance_token="mcp-conformance-test-token"
HOME="${work_dir}/home" LABBY_MCP_HTTP_TOKEN="$conformance_token" \
  LABBY_LOG="labby=warn,labby_auth=warn" \
  "${cargo_target_dir}/debug/labby" serve \
  --host 127.0.0.1 --port "$MCP_CONFORMANCE_LABBY_PORT" \
  >"${output_dir}/labby-server.log" 2>&1 &
labby_pid="$!"
labby_ready=false

for _ in $(seq 1 30); do
  if curl --fail --silent --show-error --output /dev/null \
    "http://127.0.0.1:${MCP_CONFORMANCE_LABBY_PORT}/ready"; then
    labby_ready=true
    break
  fi
  sleep 1
done

if [[ "$labby_ready" != true ]]; then
  echo "Labby MCP smoke server did not become ready" >&2
  exit 1
fi

mcp_request='{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'
unauth_status="$(curl --silent --show-error \
  --output "${output_dir}/labby-unauthenticated.json" \
  --write-out '%{http_code}' \
  --header 'Content-Type: application/json' \
  --header 'Accept: application/json, text/event-stream' \
  --data "$mcp_request" \
  "http://127.0.0.1:${MCP_CONFORMANCE_LABBY_PORT}/mcp")"
if [[ "$unauth_status" != 401 ]]; then
  echo "unauthenticated Labby MCP request returned HTTP ${unauth_status}, expected 401" >&2
  exit 1
fi

auth_status="$(curl --silent --show-error \
  --output "${output_dir}/labby-tools-list.json" \
  --write-out '%{http_code}' \
  --header "Authorization: Bearer ${conformance_token}" \
  --header 'Content-Type: application/json' \
  --header 'Accept: application/json, text/event-stream' \
  --data "$mcp_request" \
  "http://127.0.0.1:${MCP_CONFORMANCE_LABBY_PORT}/mcp")"
if [[ "$auth_status" != 200 ]]; then
  echo "authenticated Labby MCP request returned HTTP ${auth_status}, expected 200" >&2
  exit 1
fi
jq --exit-status '.jsonrpc == "2.0" and .id == 1 and (.result.tools | map(.name) | index("gateway") != null)' \
  "${output_dir}/labby-tools-list.json" >/dev/null

# Score the dated protocol against rmcp's purpose-built fixture catalog.
STATELESS=1 PORT="$MCP_CONFORMANCE_PORT" \
  "${rmcp_target_dir}/debug/conformance-server" \
  >"${output_dir}/server.log" 2>&1 &
server_pid="$!"
server_ready=false

for _ in $(seq 1 30); do
  if curl --silent --output /dev/null \
    "http://127.0.0.1:${MCP_CONFORMANCE_PORT}/mcp"; then
    server_ready=true
    break
  fi
  sleep 1
done

if [[ "$server_ready" != true ]]; then
  echo "rmcp conformance fixture server did not become ready" >&2
  exit 1
fi

"$conformance" server \
  --url "http://127.0.0.1:${MCP_CONFORMANCE_PORT}/mcp" \
  --suite all \
  --spec-version "$MCP_SPEC_VERSION" \
  --expected-failures "$dated_baseline" \
  -o "${output_dir}/server-dated"

task_scenarios=(
  tasks-lifecycle
  tasks-capability-negotiation
  tasks-wire-fields
  tasks-request-state-removal
  tasks-mrtr-input
  tasks-request-headers
  tasks-dispatch-and-envelope
  tasks-status-notifications
  tasks-required-task-error
  tasks-mrtr-composition
)

for scenario in "${task_scenarios[@]}"; do
  "$conformance" server \
    --url "http://127.0.0.1:${MCP_CONFORMANCE_PORT}/mcp" \
    --scenario "$scenario" \
    --expected-failures "$extension_baseline" \
    -o "${output_dir}/server-extensions"
done

"$conformance" client \
  --command "${rmcp_target_dir}/debug/conformance-client" \
  --suite all \
  --spec-version "$MCP_SPEC_VERSION" \
  -o "${output_dir}/client-dated"

"$conformance" client \
  --command "${rmcp_target_dir}/debug/conformance-client" \
  --suite extensions \
  --expected-failures "$extension_baseline" \
  -o "${output_dir}/client-extensions"

run_direct_proxy

echo "MCP conformance results written to ${output_dir}"
