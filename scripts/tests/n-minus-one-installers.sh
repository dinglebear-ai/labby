#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
test_root=$(mktemp -d)
trap 'rm -rf "$test_root"' EXIT

for platform in unix windows macos; do
    adapter="$repo_root/scripts/ci/n-minus-one/$platform"
    [[ -x "$adapter" ]] || { echo "missing executable adapter: $adapter" >&2; exit 1; }
    if RUNNER_TEMP="$test_root" LABBY_PREVIOUS_VERSION=v1.0.0 LABBY_CANDIDATE_VERSION=v2.0.0 \
        "$adapter" unknown >"$test_root/$platform.out" 2>"$test_root/$platform.err"; then
        echo "$platform adapter accepted an unknown stage" >&2
        exit 1
    fi
    grep -Fq 'unknown stage' "$test_root/$platform.err"
    if RUNNER_TEMP="$test_root" LABBY_PREVIOUS_VERSION=v1.0.0 LABBY_CANDIDATE_VERSION=v2.0.0 \
        "$adapter" upgrade >"$test_root/$platform-upgrade.out" 2>"$test_root/$platform-upgrade.err"; then
        echo "$platform adapter accepted an upgrade without an exact local candidate" >&2
        exit 1
    fi
    grep -Fq 'exact candidate binary is required' "$test_root/$platform-upgrade.err"
    if RUNNER_TEMP="$test_root" LABBY_PREVIOUS_VERSION=v1.0.0 LABBY_CANDIDATE_VERSION=v2.0.0 \
        LABBY_N_MINUS_ONE_CANDIDATE_BINARY="$test_root/candidate" \
        "$adapter" verify-provenance >"$test_root/$platform-provenance.out" \
        2>"$test_root/$platform-provenance.err"; then
        echo "$platform adapter accepted provenance without the attested archive" >&2
        exit 1
    fi
    grep -Fq 'exact candidate archive is required' "$test_root/$platform-provenance.err"
    for stage in verify-provenance authenticated-action restart verify-restart; do
        grep -Fq "${stage})" "$adapter" || {
            echo "$platform adapter is missing $stage" >&2
            exit 1
        }
    done
    grep -Fq 'verify-and-activate-release.sh' "$adapter" || {
        echo "$platform adapter does not use the release provenance wrapper" >&2
        exit 1
    }
    grep -Fq 'Authorization: Bearer' "$adapter" || {
        echo "$platform adapter lacks an authenticated API action" >&2
        exit 1
    }
done

echo 'installer N-1 adapter contract tests passed'
