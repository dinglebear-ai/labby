#!/usr/bin/env bash
# Verifies (default) or rewrites (--fix) the checksum/labbyVersion entities
# in unraid/labby.plg against unraid/source/ and, if given, a built release
# tarball. unraid/labby.plg drifting silently out of sync with
# unraid/source/ is the exact failure mode this exists to catch (see
# docs/runtime/UNRAID.md).
#
# Usage:
#   scripts/ci/unraid-plugin-checksums.sh [--fix] [--tag vX.Y.Z] [--tarball PATH]
#
# --tag and --tarball are optional: without them, only the eleven
# unraid/source/ companion-file checksums are checked (cheap, safe to run
# on every PR — this is what ci.yml's always-on unraid-plugin-check job
# runs). --tag/--tarball are a MANUAL tool, not wired into any CI job:
# `labbyVersion` intentionally references a specific, already-published
# labby release the operator has vetted (see "Two version numbers, on
# purpose" in docs/runtime/UNRAID.md) — it is not meant to track whatever
# tag is currently being released, and a freshly-built release tarball's
# MD5 is not reproducible build-to-build (GNU tar embeds file mtimes), so
# there is no safe way to auto-verify tarballMD5 against a same-run build.
# Run this form by hand, against a tarball downloaded from the
# already-published release you are pointing labbyVersion at, whenever you
# deliberately bump it — e.g.:
#   gh release download vX.Y.Z --repo jmagar/labby -p "lab-x86_64-unknown-linux-gnu.tar.gz"
#   scripts/ci/unraid-plugin-checksums.sh --tag vX.Y.Z --tarball lab-x86_64-unknown-linux-gnu.tar.gz --fix
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
plg="$repo_root/unraid/labby.plg"
src="$repo_root/unraid/source/usr/local/emhttp/plugins/labby"

fix=0
tag=""
tarball=""
while [ $# -gt 0 ]; do
    case "$1" in
        --fix)
            fix=1
            shift
            ;;
        --tag)
            tag="$2"
            shift 2
            ;;
        --tarball)
            tarball="$2"
            shift 2
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

entity_value() {
    # $1 = entity name. Tolerates the column-aligned whitespace used in
    # labby.plg's DTD block (e.g. "<!ENTITY name         \"labby\">").
    grep -oP "(?<=<!ENTITY $1)\s+\"\K[^\"]+" "$plg"
}

set_entity() {
    # $1 = entity name, $2 = new value. Preserves existing alignment
    # whitespace between the entity name and its opening quote.
    sed -i -E "s|(<!ENTITY $1[[:space:]]+)\"[^\"]*\"|\1\"$2\"|" "$plg"
}

mismatch=0

check_or_fix() {
    # $1 = entity name, $2 = expected value, $3 = human label
    local entity="$1" expected="$2" label="$3" current
    current="$(entity_value "$entity")"
    if [ "$current" = "$expected" ]; then
        return 0
    fi
    if [ "$fix" -eq 1 ]; then
        set_entity "$entity" "$expected"
        echo "fixed: $entity ($current -> $expected) — $label"
    else
        echo "::error::unraid/labby.plg entity '$entity' is '$current', expected '$expected' ($label). Run scripts/ci/unraid-plugin-checksums.sh --fix and commit the result." >&2
        mismatch=1
    fi
}

md5_of() { md5sum "$1" | awk '{print $1}'; }

check_or_fix cfgMD5 "$(md5_of "$src/labby.cfg")" "unraid/source/.../labby.cfg"
check_or_fix pageMD5 "$(md5_of "$src/Labby.page")" "unraid/source/.../Labby.page"
check_or_fix dashboardMD5 "$(md5_of "$src/LabbyDashboard.page")" "unraid/source/.../LabbyDashboard.page"
check_or_fix dashboardStatusMD5 "$(md5_of "$src/include/dashboard-status.php")" "unraid/source/.../include/dashboard-status.php"
check_or_fix rcMD5 "$(md5_of "$src/scripts/rc.labby")" "unraid/source/.../scripts/rc.labby"
check_or_fix preflightMD5 "$(md5_of "$src/scripts/labby-preflight.sh")" "unraid/source/.../scripts/labby-preflight.sh"
check_or_fix mountedMD5 "$(md5_of "$src/event/disks_mounted")" "unraid/source/.../event/disks_mounted"
check_or_fix unmountingMD5 "$(md5_of "$src/event/unmounting_disks")" "unraid/source/.../event/unmounting_disks"
check_or_fix incusProfileMD5 "$(md5_of "$src/incus/labby-gateway-profile.yaml")" "unraid/source/.../incus/labby-gateway-profile.yaml"
check_or_fix incusEnvMD5 "$(md5_of "$src/scripts/labby-incus-env.sh")" "unraid/source/.../scripts/labby-incus-env.sh"
check_or_fix incusInitMD5 "$(md5_of "$src/scripts/labby-incus-init.sh")" "unraid/source/.../scripts/labby-incus-init.sh"

if [ -n "$tag" ]; then
    # labbyVersion tracks the bundled labby release tag; the top-level
    # 'version' entity is the plugin package's own version and is bumped
    # independently (e.g. a packaging-only fix), so it is NOT checked here.
    check_or_fix labbyVersion "${tag#v}" "release tag $tag"
fi

if [ -n "$tarball" ]; then
    check_or_fix tarballMD5 "$(md5_of "$tarball")" "release tarball $(basename "$tarball")"
fi

if [ "$mismatch" -eq 1 ]; then
    exit 1
fi
echo "unraid/labby.plg checksums OK"
