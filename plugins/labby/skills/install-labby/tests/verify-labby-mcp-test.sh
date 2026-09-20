#!/usr/bin/env bash
set -euo pipefail

skill_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
verifier="$skill_dir/scripts/verify-labby-mcp.sh"
fixture_dir=$(mktemp -d "${TMPDIR:-/tmp}/verify-labby-mcp-test.XXXXXX")
trap 'rm -rf -- "$fixture_dir"' EXIT

cat >"$fixture_dir/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
output=
method=
while [[ $# -gt 0 ]]; do
  case "$1" in
    --output) output=$2; shift 2 ;;
    --header)
      if [[ $2 == Mcp-Method:* ]]; then method=${2#Mcp-Method: }; fi
      shift 2
      ;;
    --write-out|--config|--data|--connect-timeout|--max-time) shift 2 ;;
    *) shift ;;
  esac
done

case "$method" in
  tools/list)
    printf '%s' '{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"gateway"},{"name":"doctor"}]}}' >"$output"
    ;;
  tools/call)
    printf '%s' '{"jsonrpc":"2.0","id":2,"result":{"isError":false,"content":[{"type":"text","text":"ok"}]}}' >"$output"
    ;;
  *) exit 70 ;;
esac
printf 200
EOF
chmod +x "$fixture_dir/curl"

cat >"$fixture_dir/labby" <<'EOF'
#!/usr/bin/env bash
printf 'labby 1.20.1\n'
EOF
chmod +x "$fixture_dir/labby"

output=$(PATH="$fixture_dir:$PATH" LABBY_EXPECTED_VERSION=1.20.1 LABBY_BIN=labby \
  "$verifier" http://127.0.0.1:8765/mcp)
[[ "$output" == 'Labby MCP verification passed: protocol=2025-06-18 tools=2 gateway.help=ok' ]]

if PATH="$fixture_dir:$PATH" LABBY_EXPECTED_VERSION=1.20.0 LABBY_BIN=labby \
  "$verifier" http://127.0.0.1:8765/mcp >"$fixture_dir/out" 2>"$fixture_dir/err"; then
  echo "version mismatch unexpectedly passed" >&2
  exit 1
fi
grep -F 'expected labby 1.20.0, got labby 1.20.1' "$fixture_dir/err" >/dev/null

if PATH="$fixture_dir:$PATH" LABBY_MCP_TOKEN=$'unsafe\nheader' \
  "$verifier" http://127.0.0.1:8765/mcp >"$fixture_dir/out" 2>"$fixture_dir/err"; then
  echo "multiline token unexpectedly passed" >&2
  exit 1
fi
grep -F 'LABBY_MCP_TOKEN must be a single line' "$fixture_dir/err" >/dev/null

if PATH="$fixture_dir:$PATH" LABBY_MCP_CONNECT_TIMEOUT=0 \
  "$verifier" http://127.0.0.1:8765/mcp >"$fixture_dir/out" 2>"$fixture_dir/err"; then
  echo "invalid connect timeout unexpectedly passed" >&2
  exit 1
fi
grep -F 'LABBY_MCP_CONNECT_TIMEOUT must be a positive integer' "$fixture_dir/err" >/dev/null

if PATH="$fixture_dir:$PATH" LABBY_MCP_MAX_TIME=forever \
  "$verifier" http://127.0.0.1:8765/mcp >"$fixture_dir/out" 2>"$fixture_dir/err"; then
  echo "invalid overall timeout unexpectedly passed" >&2
  exit 1
fi
grep -F 'LABBY_MCP_MAX_TIME must be a positive integer' "$fixture_dir/err" >/dev/null

echo 'verify-labby-mcp tests passed'
