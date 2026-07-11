#!/usr/bin/env bash
set -euo pipefail

env_file="${LABBY_PALETTE_ENV_FILE:-${1:-}}"
if [[ -n "${env_file}" ]]; then
  # shellcheck disable=SC1090
  set -a && source "${env_file}" && set +a
fi

ssh_target="${LABBY_PALETTE_WINDOWS_SSH:-}"
remote_dir="${LABBY_PALETTE_WINDOWS_DIR:-}"
exe="${LABBY_PALETTE_EXE:-}"
evidence_local="${LABBY_PALETTE_EVIDENCE_DIR:-}"
token="${LABBY_PALETTE_TOKEN:-${LABBY_MCP_HTTP_TOKEN:-${LAB_MCP_HTTP_TOKEN:-}}}"

[[ -n "${ssh_target}" ]] || { echo "LABBY_PALETTE_WINDOWS_SSH is required" >&2; exit 2; }
[[ -n "${remote_dir}" ]] || { echo "LABBY_PALETTE_WINDOWS_DIR is required" >&2; exit 2; }
[[ -n "${exe}" && -f "${exe}" ]] || { echo "LABBY_PALETTE_EXE must point to a built Windows palette exe" >&2; exit 2; }
[[ -n "${env_file}" && -f "${env_file}" ]] || { echo "pass an env file or set LABBY_PALETTE_ENV_FILE" >&2; exit 2; }
[[ -n "${token}" ]] || { echo "LABBY_PALETTE_TOKEN or LABBY_MCP_HTTP_TOKEN is required" >&2; exit 2; }
[[ -n "${evidence_local}" ]] || evidence_local="$(pwd)/palette-agent-os-evidence"

mkdir -p "${evidence_local}"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT
sanitized_env="${tmp_dir}/palette-smoke.env"
grep -Ev '^[[:space:]]*(LABBY_PALETTE_TOKEN|LABBY_MCP_HTTP_TOKEN|LAB_MCP_HTTP_TOKEN)[[:space:]]*=' "${env_file}" > "${sanitized_env}" || true

ssh "${ssh_target}" "rm -rf '${remote_dir}' && mkdir -p '${remote_dir}/scripts' '${remote_dir}/evidence'"
scp "${exe}" "${ssh_target}:${remote_dir}/labby-palette-tauri.exe"
scp "${sanitized_env}" "${ssh_target}:${remote_dir}/palette-smoke.env"
scp "$(dirname "$0")/desktop-smoke.ps1" "${ssh_target}:${remote_dir}/scripts/desktop-smoke.ps1"
printf '%s\n' "${token}" | ssh "${ssh_target}" "umask 077 && cat > '${remote_dir}/palette-smoke.token'"

ssh "${ssh_target}" 'bash -s' -- "${remote_dir}" <<'REMOTE'
set -euo pipefail
remote_dir="$1"
cd "${remote_dir}"
cleanup() {
  rm -f palette-smoke.env palette-smoke.remote.env palette-smoke.token
}
trap cleanup EXIT
token="$(cat palette-smoke.token)"
cp palette-smoke.env palette-smoke.remote.env
printf '\nLABBY_PALETTE_EXE=%s\nLABBY_PALETTE_EVIDENCE_DIR=%s\n' \
  "${remote_dir}/labby-palette-tauri.exe" \
  "${remote_dir}/evidence" >> palette-smoke.remote.env
export LABBY_PALETTE_TOKEN="${token}"
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/desktop-smoke.ps1 -EnvFile palette-smoke.remote.env
REMOTE
scp "${ssh_target}:${remote_dir}/evidence/*" "${evidence_local}/" || true
echo "agent-os smoke evidence: ${evidence_local}"
