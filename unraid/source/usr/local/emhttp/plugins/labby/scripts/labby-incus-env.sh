#!/bin/bash
# labby-incus-env.sh - sourced by labby-incus-init.sh and rc.labby when
# RUNTIME_MODE="incus". Points the `incus` client at incus-unraid's
# private-prefixed daemon, not a system-wide Incus install.

INCUS_PREFIX="${INCUS_PREFIX:-/usr/local/incus}"

if [ ! -x "${INCUS_PREFIX}/bin/incus" ]; then
    echo "labby-incus-env: ${INCUS_PREFIX}/bin/incus not found - install the incus-unraid plugin first" >&2
    if (return 0 2>/dev/null); then
        return 1
    fi
    exit 1
fi

case ":${LD_LIBRARY_PATH:-}:" in
    *":${INCUS_PREFIX}/lib:"*) ;;
    *) export LD_LIBRARY_PATH="${INCUS_PREFIX}/lib${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}" ;;
esac

case ":${PATH}:" in
    *":${INCUS_PREFIX}/libexec/incus:"*) ;;
    *) export PATH="${INCUS_PREFIX}/libexec/incus:${PATH}" ;;
esac
case ":${PATH}:" in
    *":${INCUS_PREFIX}/bin:"*) ;;
    *) export PATH="${INCUS_PREFIX}/bin:${PATH}" ;;
esac
export INCUS_DIR="/mnt/user/appdata/incus"
