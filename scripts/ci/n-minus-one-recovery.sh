#!/usr/bin/env bash
# Offline recovery of the baseline state, before reactivating its older binary.
# The caller owns stopping the service. Linux service adapters pass the state
# owner so recovery uses the same uid as the daemon, never a root override.
set -euo pipefail

operation=${1:?capture or restore required}
binary=${2:?verified candidate binary required}
installation=${3:?installation root required}
recovery=${4:?recovery directory required}
owner=${5:-}
export LABBY_HOME="$installation" LABBY_RECOVERY_KEY_PATH="$recovery/key"

run_candidate() {
    if [[ -n "$owner" ]]; then
        exec runuser -u "$owner" -- "$recovery/candidate" "$@"
    fi
    exec "$binary" "$@"
}

case "$operation" in
    capture)
        umask 077
        # A repeated capture must not overwrite the only pre-upgrade snapshot.
        mkdir "$recovery"
        dd if=/dev/urandom of="$LABBY_RECOVERY_KEY_PATH" bs=32 count=1 2>/dev/null
        if [[ -n "$owner" ]]; then
            # The verified input may live below a runner/root-only directory.
            # Stage it outside the installation, in this new private directory,
            # before dropping privileges; restore reuses the same candidate.
            install -m 0555 "$binary" "$recovery/candidate"
            chown "$owner" "$recovery" "$LABBY_RECOVERY_KEY_PATH"
        fi
        # A parent default ACL can override umask during creation. Enforce the
        # candidate's owner-only key and directory contract after chown.
        chmod 0700 "$recovery"
        chmod 0600 "$LABBY_RECOVERY_KEY_PATH"
        run_candidate state export --output "$recovery/bundle"
        ;;
    restore)
        # Restore verifies the HMAC and takes the daemon's lifecycle lock.
        run_candidate state restore --bundle "$recovery/bundle"
        ;;
    *) echo "unknown recovery operation: $operation" >&2; exit 64 ;;
esac
