#!/usr/bin/env bash
set -euo pipefail

deployment=${1:?deployment path required}
previous=${2:?previous release required}
candidate=${3:?candidate release required}
case "$deployment" in unix|windows|macos|compose|incus|host-service) ;; *) exit 64 ;; esac

# Platform-specific commands are supplied by the workflow/fixture. The common
# contract guarantees the same stateful sequence and refuses missing stages.
stages=(install_previous seed_state verify_previous verify_provenance upgrade verify_candidate authenticated_action restart verify_restart rollback)
post_rollback_stages=(restart verify_rollback authenticated_action)
for stage in "${stages[@]}" "${post_rollback_stages[@]}"; do
  upper_stage=$(printf '%s' "$stage" | tr '[:lower:]' '[:upper:]')
  variable="LABBY_N_MINUS_ONE_${upper_stage}"
  command=${!variable:-}
  [[ -n "$command" ]] || { echo "missing $variable for $deployment" >&2; exit 1; }
  LABBY_DEPLOYMENT=$deployment LABBY_PREVIOUS_VERSION=$previous LABBY_CANDIDATE_VERSION=$candidate bash -euo pipefail -c "$command"
done
