#!/usr/bin/env bash
# Offline recovery of the baseline state, before reactivating its older binary.
# The caller owns stopping the service and preserving file ownership afterwards.
set -euo pipefail

operation=${1:?capture or restore required}
binary=${2:?verified candidate binary required}
installation=${3:?installation root required}
recovery=${4:?recovery directory required}
export LABBY_HOME="$installation" LABBY_RECOVERY_KEY_PATH="$recovery/key"

case "$operation" in
    capture)
        umask 077
        # A repeated capture must not overwrite the only pre-upgrade snapshot.
        mkdir "$recovery"
        dd if=/dev/urandom of="$LABBY_RECOVERY_KEY_PATH" bs=32 count=1 2>/dev/null
        exec "$binary" state export --output "$recovery/bundle"
        ;;
    restore)
        # Restore verifies the HMAC and takes the daemon's lifecycle lock.
        exec "$binary" state restore --bundle "$recovery/bundle"
        ;;
    *) echo "unknown recovery operation: $operation" >&2; exit 64 ;;
esac
