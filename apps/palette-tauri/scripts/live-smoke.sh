#!/usr/bin/env bash
set -euo pipefail

env_file="${LABBY_PALETTE_ENV_FILE:-${1:-}}"
if [[ -n "${env_file}" ]]; then
  # shellcheck disable=SC1090
  set -a && source "${env_file}" && set +a
fi

api_url="${LABBY_PALETTE_API_URL:-${LABBY_API_URL:-}}"
token="${LABBY_PALETTE_TOKEN:-${LABBY_MCP_HTTP_TOKEN:-${LAB_MCP_HTTP_TOKEN:-}}}"
query="${LABBY_PALETTE_QUERY:-gateway}"
execute_id="${LABBY_PALETTE_EXECUTE_ID:-}"
execute_params="${LABBY_PALETTE_EXECUTE_PARAMS:-{}}"

[[ -n "${api_url}" ]] || { echo "LABBY_PALETTE_API_URL or LABBY_API_URL is required" >&2; exit 2; }
[[ -n "${token}" ]] || { echo "LABBY_PALETTE_TOKEN or LABBY_MCP_HTTP_TOKEN is required" >&2; exit 2; }

api_url="${api_url%/}"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT

curl_json() {
  curl -fsS \
    -H "Authorization: Bearer ${token}" \
    -H "Accept: application/json" \
    "$@"
}

catalog="${tmp_dir}/catalog.json"
curl_json --get "${api_url}/v1/palette/search" \
  --data-urlencode "q=${query}" \
  --data-urlencode "limit=10" > "${catalog}"

python3 - "${catalog}" "${query}" <<'PY'
import json, sys
path, query = sys.argv[1:3]
catalog = json.load(open(path, encoding="utf-8"))
entries = catalog.get("entries") if isinstance(catalog.get("entries"), list) else []
if not entries:
    raise SystemExit(f"query {query!r} returned no launcher rows")
print(json.dumps({
    "ok": True,
    "query": query,
    "entries": len(entries),
    "first": entries[0].get("id"),
    "fingerprint": catalog.get("fingerprint"),
}, indent=2))
PY

first_id="$(python3 - "${catalog}" <<'PY'
import json, sys
catalog = json.load(open(sys.argv[1], encoding="utf-8"))
entries = catalog.get("entries") if isinstance(catalog.get("entries"), list) else []
print(entries[0].get("id", "") if entries else "")
PY
)"

if [[ -n "${first_id}" ]]; then
  encoded_id="$(python3 - "${first_id}" <<'PY'
import sys, urllib.parse
print(urllib.parse.quote(sys.argv[1], safe=""))
PY
)"
  curl_json "${api_url}/v1/palette/schema?id=${encoded_id}" > "${tmp_dir}/schema.json"
  python3 - "${tmp_dir}/schema.json" <<'PY'
import json, sys
schema = json.load(open(sys.argv[1], encoding="utf-8"))
if not schema.get("id"):
    raise SystemExit("schema response omitted id")
print(json.dumps({"schemaId": schema.get("id"), "hasSchema": bool(schema.get("inputSchema"))}, indent=2))
PY
fi

if [[ -n "${execute_id}" ]]; then
  python3 - "${execute_id}" "${execute_params}" > "${tmp_dir}/execute-body.json" <<'PY'
import json, sys
id_, params = sys.argv[1:3]
print(json.dumps({"id": id_, "params": json.loads(params), "confirmDestructive": False}))
PY
  curl_json \
    -X POST \
    -H "Content-Type: application/json" \
    --data @"${tmp_dir}/execute-body.json" \
    "${api_url}/v1/palette/execute" > "${tmp_dir}/execute.json"
  python3 - "${tmp_dir}/execute.json" <<'PY'
import json, sys
result = json.load(open(sys.argv[1], encoding="utf-8"))
if not result.get("id"):
    raise SystemExit("execute response omitted id")
print(json.dumps({"executed": result.get("id")}, indent=2))
PY
fi
