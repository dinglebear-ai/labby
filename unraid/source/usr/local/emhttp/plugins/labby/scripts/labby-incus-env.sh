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
export INCUS="${INCUS:-${INCUS_PREFIX}/bin/incus}"

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

# The private Incus library path also supplies libxtables. Keep Unraid's
# system iptables extensions discoverable when this environment is active.
SYSTEM_XTABLES_LIBDIR="${SYSTEM_XTABLES_LIBDIR:-/usr/lib64/xtables}"
if [ -d "$SYSTEM_XTABLES_LIBDIR" ]; then
    export XTABLES_LIBDIR="${XTABLES_LIBDIR:-$SYSTEM_XTABLES_LIBDIR}"
fi

# The Incus plugin owns the daemon state path. Read only INCUS_DIR in a
# subshell so sourcing its config cannot overwrite Labby's SERVICE or other
# runtime settings in the caller.
INCUS_CONFIG="${INCUS_CONFIG:-/boot/config/plugins/incus/incus.cfg}"
incus_configured_dir=""
if [ -r "$INCUS_CONFIG" ]; then
    incus_configured_dir="$(
        unset INCUS_DIR
        # shellcheck disable=SC1090
        . "$INCUS_CONFIG" || exit 1
        printf '%s' "${INCUS_DIR:-}"
    )" || {
        echo "labby-incus-env: failed to read INCUS_DIR from ${INCUS_CONFIG}" >&2
        if (return 0 2>/dev/null); then
            return 1
        fi
        exit 1
    }
fi
export INCUS_DIR="${incus_configured_dir:-${INCUS_DIR:-/mnt/user/appdata/incus}}"
