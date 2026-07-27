#!/usr/bin/env bash
set -euo pipefail

# Reproduce the upstream rmcp 3.0.0-beta.2 conformance gate against the
# 2026-07-28 dated protocol and the separately scored extension suite.
#
# JavaScript dependencies are installed exactly once before any scenario runs.
# This avoids concurrent npx cache mutation when scenarios are executed in CI.

RMCP_VERSION="${RMCP_VERSION:-3.0.0-beta.2}"
RMCP_TAG="${RMCP_TAG:-rmcp-v${RMCP_VERSION}}"
RMCP_COMMIT="${RMCP_COMMIT:-14298b72e0b25473ea79d5465fe186e22eb86397}"
MCP_CONFORMANCE_VERSION="${MCP_CONFORMANCE_VERSION:-0.2.0-alpha.9}"
MCP_SPEC_VERSION="${MCP_SPEC_VERSION:-2026-07-28}"
MCP_CONFORMANCE_PORT="${MCP_CONFORMANCE_PORT:-18002}"
MCP_CONFORMANCE_LABBY_PORT="${MCP_CONFORMANCE_LABBY_PORT:-18003}"
MCP_CONFORMANCE_OUTPUT_DIR="${MCP_CONFORMANCE_OUTPUT_DIR:-target/mcp-conformance}"

repo_root="$(git rev-parse --show-toplevel)"
output_dir="${repo_root}/${MCP_CONFORMANCE_OUTPUT_DIR}"
baseline="${repo_root}/conformance/expected-failures-extensions.yaml"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/labby-mcp-conformance.XXXXXX")"
server_pid=""
labby_pid=""

cleanup() {
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

if ! grep -Eq "rmcp = \\{ version = \"=${RMCP_VERSION}\"" "${repo_root}/Cargo.toml"; then
  echo "Cargo.toml must pin rmcp exactly to =${RMCP_VERSION}" >&2
  exit 1
fi

mkdir -p "$output_dir"

git clone --quiet --depth 1 --branch "$RMCP_TAG" \
  https://github.com/modelcontextprotocol/rust-sdk.git \
  "${work_dir}/rust-sdk"

actual_commit="$(git -C "${work_dir}/rust-sdk" rev-parse HEAD)"
if [[ "$actual_commit" != "$RMCP_COMMIT" ]]; then
  echo "rmcp tag ${RMCP_TAG} resolved to ${actual_commit}, expected ${RMCP_COMMIT}" >&2
  exit 1
fi

npm install \
  --prefix "${work_dir}/js" \
  --ignore-scripts \
  --no-audit \
  --no-fund \
  "@modelcontextprotocol/conformance@${MCP_CONFORMANCE_VERSION}"

conformance="${work_dir}/js/node_modules/.bin/conformance"

RUSTFLAGS="" cargo build \
  --manifest-path "${work_dir}/rust-sdk/Cargo.toml" \
  -p mcp-conformance
RUSTFLAGS="" cargo build -p labby --all-features --locked

# The conformance CLI has no header option. Keep Labby authenticated in this
# test and put a loopback-only proxy in front of it which injects the fixed
# test token. The suites therefore exercise Labby's real HTTP transport,
# stateless handler, and auth boundary rather than rmcp's reference server.
conformance_token="mcp-conformance-test-token"
HOME="${work_dir}/home" LABBY_MCP_HTTP_TOKEN="$conformance_token" \
  LABBY_LOG="labby=warn,labby_auth=warn" \
  "${repo_root}/target/debug/labby" serve \
  --host 127.0.0.1 --port "$MCP_CONFORMANCE_LABBY_PORT" \
  >"${output_dir}/labby-server.log" 2>&1 &
labby_pid="$!"
MCP_CONFORMANCE_UPSTREAM_PORT="$MCP_CONFORMANCE_LABBY_PORT" \
  MCP_CONFORMANCE_TOKEN="$conformance_token" \
  MCP_CONFORMANCE_PORT="$MCP_CONFORMANCE_PORT" \
  node -e '
    const http = require("http");
    const port = Number(process.env.MCP_CONFORMANCE_PORT);
    const upstreamPort = Number(process.env.MCP_CONFORMANCE_UPSTREAM_PORT);
    const token = process.env.MCP_CONFORMANCE_TOKEN;
    http.createServer((req, res) => {
      const upstream = http.request({
        host: "127.0.0.1", port: upstreamPort, path: req.url,
        method: req.method, headers: {...req.headers, authorization: `Bearer ${token}`},
      }, upstreamResponse => { res.writeHead(upstreamResponse.statusCode, upstreamResponse.headers); upstreamResponse.pipe(res); });
      upstream.on("error", error => { res.writeHead(502); res.end(String(error)); });
      req.pipe(upstream);
    }).listen(port, "127.0.0.1");
  ' >"${output_dir}/server.log" 2>&1 &
server_pid="$!"
server_ready=false

for _ in $(seq 1 30); do
  if curl --fail --silent --show-error --output /dev/null \
    "http://127.0.0.1:${MCP_CONFORMANCE_LABBY_PORT}/ready"; then
    server_ready=true
    break
  fi
  sleep 1
done

if [[ "$server_ready" != true ]]; then
  echo "Labby conformance server did not become ready" >&2
  exit 1
fi

"$conformance" server \
  --url "http://127.0.0.1:${MCP_CONFORMANCE_PORT}/mcp" \
  --suite all \
  --spec-version "$MCP_SPEC_VERSION" \
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
    --expected-failures "$baseline" \
    -o "${output_dir}/server-extensions"
done

"$conformance" client \
  --command "${work_dir}/rust-sdk/target/debug/conformance-client" \
  --suite all \
  --spec-version "$MCP_SPEC_VERSION" \
  -o "${output_dir}/client-dated"

"$conformance" client \
  --command "${work_dir}/rust-sdk/target/debug/conformance-client" \
  --suite extensions \
  --expected-failures "$baseline" \
  -o "${output_dir}/client-extensions"

echo "MCP conformance results written to ${output_dir}"
