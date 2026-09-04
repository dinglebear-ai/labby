#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
bootstrap="$repo_root/scripts/incus-bootstrap.sh"
functions=$(awk '/^usage\(\)/ { exit } { print }' "$bootstrap")
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT

for label in storage profile container-launch container-start backup-config hostname binary provision readiness tailscale-key tailscale-up tailscale-cleanup; do
  case_root="$root/$label"
  mkdir -p "$case_root/tx"
  printf 'candidate\n' >"$case_root/state"
  (
    eval "$functions"
    DRY_RUN=0
    TRANSACTION_DIR="$case_root/tx"
    ROLLBACK_FILE="$TRANSACTION_DIR/rollback.commands"
    RESIDUAL_REPORT="$case_root/residual"
    TRANSACTION_COMMITTED=0
    TS_AUTHKEY_STAGED=0
    printf "echo prior >'%s'\n" "$case_root/state" >"$ROLLBACK_FILE"
    trap 'transaction_exit $?' EXIT
    LABBY_INCUS_FAIL_AFTER="$label"
    checkpoint "$label"
  ) >/dev/null 2>&1 && { echo "checkpoint $label unexpectedly succeeded" >&2; exit 1; }
  grep -qx prior "$case_root/state"
done

case_root="$root/compound"
mkdir -p "$case_root/tx"
(
  eval "$functions"
  DRY_RUN=0
  TRANSACTION_DIR="$case_root/tx"
  ROLLBACK_FILE="$TRANSACTION_DIR/rollback.commands"
  RESIDUAL_REPORT="$case_root/residual"
  TRANSACTION_COMMITTED=0
  TS_AUTHKEY_STAGED=0
  printf "printf restored >'%s'\nfalse\n" "$case_root/restored" >"$ROLLBACK_FILE"
  rollback_transaction 1
) >/dev/null 2>&1 && { echo 'compound rollback unexpectedly succeeded' >&2; exit 1; }
grep -qx restored "$case_root/restored"
test -s "$case_root/residual"
test "$(stat -f '%Lp' "$case_root/residual" 2>/dev/null || stat -c '%a' "$case_root/residual")" = 600

echo 'incus bootstrap transaction tests passed'
