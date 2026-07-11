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

[[ -n "${ssh_target}" ]] || { echo "LABBY_PALETTE_WINDOWS_SSH is required" >&2; exit 2; }
[[ -n "${remote_dir}" ]] || { echo "LABBY_PALETTE_WINDOWS_DIR is required" >&2; exit 2; }
[[ -n "${exe}" && -f "${exe}" ]] || { echo "LABBY_PALETTE_EXE must point to a built Windows palette exe" >&2; exit 2; }
[[ -n "${env_file}" && -f "${env_file}" ]] || { echo "pass an env file or set LABBY_PALETTE_ENV_FILE" >&2; exit 2; }
[[ -n "${evidence_local}" ]] || evidence_local="$(pwd)/palette-agent-os-evidence"

mkdir -p "${evidence_local}"

ssh "${ssh_target}" "rm -rf '${remote_dir}' && mkdir -p '${remote_dir}/scripts' '${remote_dir}/evidence'"
scp "${exe}" "${ssh_target}:${remote_dir}/labby-palette-tauri.exe"
scp "${env_file}" "${ssh_target}:${remote_dir}/palette-smoke.env"
scp "$(dirname "$0")/desktop-smoke.ps1" "${ssh_target}:${remote_dir}/scripts/desktop-smoke.ps1"

ssh "${ssh_target}" "cd '${remote_dir}' && cp palette-smoke.env palette-smoke.remote.env && printf '\nLABBY_PALETTE_EXE=%s\nLABBY_PALETTE_EVIDENCE_DIR=%s\n' '${remote_dir}/labby-palette-tauri.exe' '${remote_dir}/evidence' >> palette-smoke.remote.env && powershell -NoProfile -ExecutionPolicy Bypass -File scripts/desktop-smoke.ps1 -EnvFile palette-smoke.remote.env"
scp "${ssh_target}:${remote_dir}/evidence/*" "${evidence_local}/" || true
echo "agent-os smoke evidence: ${evidence_local}"
