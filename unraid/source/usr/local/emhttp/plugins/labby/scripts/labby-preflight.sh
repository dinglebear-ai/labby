#!/bin/bash
# Read-only capability check for the labby gateway binary. Fails loud, changes
# nothing — rc.labby refuses to start() if this fails.
set -u

EMHTTP="/usr/local/emhttp/plugins/labby"
BIN="${EMHTTP}/bin/labby"

fail() { echo "labby-preflight: $*" >&2; exit 1; }

[ -x "$BIN" ] || fail "binary missing or not executable at $BIN"

# labby's release binary is dynamically linked against glibc, built in a
# Debian container; verified empirically to run on Unraid 7.x (glibc 2.43+).
# This just catches a much older Unraid base early with a clear message
# instead of a raw "version GLIBC_2.39 not found" crash.
GLIBC_VER="$(ldd --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+$')"
if [ -n "$GLIBC_VER" ]; then
    MAJOR="${GLIBC_VER%%.*}"
    MINOR="${GLIBC_VER##*.}"
    if [ "$MAJOR" -lt 2 ] || { [ "$MAJOR" -eq 2 ] && [ "$MINOR" -lt 39 ]; }; then
        fail "glibc $GLIBC_VER is older than the required 2.39 (labby needs a newer Unraid base)"
    fi
fi

command -v curl >/dev/null 2>&1 || fail "curl not found on PATH (needed for readiness polling)"

exit 0
