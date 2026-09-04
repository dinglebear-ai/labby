#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
test_root=$(mktemp -d)
trap 'rm -rf "$test_root"' EXIT

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

assert_contains() {
    local file=$1 expected=$2
    grep -Fq -- "$expected" "$file" || fail "$file did not contain: $expected"
}

file_mode() {
    if stat -c '%a' "$1" >/dev/null 2>&1; then
        stat -c '%a' "$1"
    else
        stat -f '%Lp' "$1"
    fi
}

make_release() {
    local fixtures=$1 tag=$2 body=$3
    local release_dir="$fixtures/releases/$tag"
    local archive_root="$test_root/archive-$tag"
    mkdir -p "$release_dir" "$archive_root"
    printf '#!/bin/sh\nprintf "%%s\\n" %s\n' "$(printf %q "$body")" >"$archive_root/labby"
    chmod 755 "$archive_root/labby"
    tar -czf "$release_dir/lab-x86_64-unknown-linux-gnu.tar.gz" -C "$archive_root" labby
    shasum -a 256 "$release_dir/lab-x86_64-unknown-linux-gnu.tar.gz" |
        awk '{print $1 "  lab-x86_64-unknown-linux-gnu.tar.gz"}' \
            >"$release_dir/lab-x86_64-unknown-linux-gnu.tar.gz.sha256"
}

make_fake_tools() {
    local bin=$1 fixtures=$2
    mkdir -p "$bin"
    cat >"$bin/uname" <<'EOF'
#!/bin/sh
case "${1:-}" in
  -s) echo Linux ;;
  -m) echo x86_64 ;;
  *) echo Linux ;;
esac
EOF
    cat >"$bin/curl" <<'EOF'
#!/bin/sh
out=
url=
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) out=$2; shift 2 ;;
        -*) shift ;;
        *) url=$1; shift ;;
    esac
done
[ -z "${LABBY_TEST_CURL_LOG:-}" ] || printf '%s\n' "$url" >>"$LABBY_TEST_CURL_LOG"
case "$url" in
    */releases\?per_page=20) source_path="$LABBY_TEST_FIXTURES/releases.json" ;;
    */releases/download/*)
        suffix=${url#*/releases/download/}
        source_path="$LABBY_TEST_FIXTURES/releases/$suffix"
        ;;
    *) exit 22 ;;
esac
[ -f "$source_path" ] || exit 22
if [ -n "$out" ]; then cp "$source_path" "$out"; else cat "$source_path"; fi
EOF
    cat >"$bin/gh" <<'EOF'
#!/bin/sh
case "$*" in
  "attestation verify "*) exit 0 ;;
  *) exit 64 ;;
esac
EOF
    chmod 755 "$bin/uname" "$bin/curl" "$bin/gh"
}

test_root_installer_is_self_contained_when_piped_from_arbitrary_cwd() {
    local case_root="$test_root/root-pipe"
    local fixtures="$case_root/fixtures" fake_bin="$case_root/fake-bin" home="$case_root/home"
    mkdir -p "$fixtures" "$home" "$case_root/cwd"
    make_release "$fixtures" v1.0.0 release-v1
    make_fake_tools "$fake_bin" "$fixtures"
    (
        cd "$case_root/cwd"
        env -i HOME="$home" PATH="$fake_bin:/usr/bin:/bin" LABBY_TEST_FIXTURES="$fixtures" \
            LABBY_INSTALL_DIR="$home/bin" LABBY_INSTALL_REPO=example/labby \
            LABBY_INSTALL_VERSION=v1.0.0 /bin/sh <"$repo_root/install.sh"
    )
    [ "$("$home/bin/labby")" = release-v1 ] || fail "piped root installer was not self-contained"
}

test_latest_api_failure_never_uses_mutable_latest_download() {
    local case_root="$test_root/latest-api"
    local fixtures="$case_root/fixtures" fake_bin="$case_root/fake-bin" home="$case_root/home"
    mkdir -p "$fixtures" "$home"
    make_fake_tools "$fake_bin" "$fixtures"
    if run_installer "$home" "$fixtures" "$fake_bin" LABBY_TEST_CURL_LOG="$case_root/curl.log" \
        >"$case_root/out" 2>"$case_root/err"; then
        fail "latest resolution unexpectedly succeeded without an immutable tag"
    fi
    if grep -Fq '/releases/latest/download' "$case_root/curl.log"; then
        fail "installer used mutable releases/latest after API failure"
    fi
}

test_release_failure_matrix_preserves_existing_binary() {
    local case_root="$test_root/failure-matrix"
    local fixtures="$case_root/fixtures" fake_bin="$case_root/fake-bin" home="$case_root/home"
    mkdir -p "$fixtures" "$home/bin"
    make_release "$fixtures" v1.0.0 release-v1
    make_fake_tools "$fake_bin" "$fixtures"
    printf '#!/bin/sh\necho sentinel\n' >"$home/bin/labby"; chmod 755 "$home/bin/labby"

    mv "$fixtures/releases/v1.0.0/lab-x86_64-unknown-linux-gnu.tar.gz" "$case_root/archive"
    if run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_VERSION=v1.0.0 >"$case_root/out" 2>"$case_root/err"; then
        fail "missing release asset unexpectedly succeeded"
    fi
    [ "$("$home/bin/labby")" = sentinel ] || fail "asset failure replaced existing binary"
    mv "$case_root/archive" "$fixtures/releases/v1.0.0/lab-x86_64-unknown-linux-gnu.tar.gz"

    mv "$fixtures/releases/v1.0.0/lab-x86_64-unknown-linux-gnu.tar.gz.sha256" "$case_root/sidecar"
    if run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_VERSION=v1.0.0 >"$case_root/out" 2>"$case_root/err"; then
        fail "missing checksum sidecar unexpectedly succeeded"
    fi
    assert_contains "$case_root/err" "require checksum verification"
    [ "$("$home/bin/labby")" = sentinel ] || fail "sidecar failure replaced existing binary"
    mv "$case_root/sidecar" "$fixtures/releases/v1.0.0/lab-x86_64-unknown-linux-gnu.tar.gz.sha256"

    printf 'not-a-digest  lab-x86_64-unknown-linux-gnu.tar.gz\n' \
        >"$fixtures/releases/v1.0.0/lab-x86_64-unknown-linux-gnu.tar.gz.sha256"
    if run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_VERSION=v1.0.0 >"$case_root/out" 2>"$case_root/err"; then
        cat "$case_root/err" >&2
        fail "malformed checksum unexpectedly succeeded"
    fi
    assert_contains "$case_root/err" "checksum verification FAILED"
    [ "$("$home/bin/labby")" = sentinel ] || fail "malformed checksum replaced existing binary"
}

test_checksum_ignores_sidecar_subject_and_hashes_requested_archive() {
    local case_root="$test_root/checksum-subject"
    local archive="$case_root/requested.tar.gz" other="$case_root/other.tar.gz" sidecar="$case_root/checksum"
    mkdir -p "$case_root"
    printf 'requested bytes' >"$archive"
    printf 'other bytes' >"$other"
    shasum -a 256 "$other" | awk '{print $1 "  other.tar.gz"}' >"$sidecar"
    sed '$d' "$repo_root/scripts/install.sh" >"$case_root/functions.sh"
    if (cd "$case_root" && . "$case_root/functions.sh"; sha256_check "$archive" "$sidecar"); then
        fail "checksum accepted the digest of the sidecar filename instead of the requested archive"
    fi
}

test_cached_artifact_is_rehashed_before_reuse() {
    local case_root="$test_root/cache-tamper"
    local fixtures="$case_root/fixtures" fake_bin="$case_root/fake-bin" home="$case_root/home"
    mkdir -p "$fixtures" "$home"
    make_release "$fixtures" v1.0.0 release-v1
    make_fake_tools "$fake_bin" "$fixtures"
    run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_VERSION=v1.0.0 >/dev/null 2>&1
    artifact=$(find "$home/bin/.labby-install/artifacts" -type f -name labby)
    printf '#!/bin/sh\necho tampered\n' >"$artifact"
    chmod 755 "$artifact"
    if run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_VERSION=v1.0.0 \
        >"$case_root/out" 2>"$case_root/err"; then
        fail "installer reused a tampered cached artifact"
    fi
    assert_contains "$case_root/err" "cached artifact digest"
}

test_local_candidate_requires_and_records_exact_digest() {
    local case_root="$test_root/local"
    local fixtures="$case_root/fixtures" fake_bin="$case_root/fake-bin" home="$case_root/home"
    mkdir -p "$fixtures" "$home" "$fake_bin"
    make_fake_tools "$fake_bin" "$fixtures"
    local candidate="$case_root/labby"
    printf '#!/bin/sh\necho candidate\n' >"$candidate"; chmod 755 "$candidate"
    digest=$(shasum -a 256 "$candidate" | awk '{print $1}')
    run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_LOCAL_BINARY="$candidate" \
        LABBY_INSTALL_LOCAL_SHA256="$digest" LABBY_INSTALL_VERSION=v2.0.0 >/dev/null 2>&1
    [ "$("$home/bin/labby")" = candidate ] || fail "exact local candidate was not installed"
    assert_contains "$home/bin/.labby-install/receipt" "source=local"
    assert_contains "$home/bin/.labby-install/receipt" "sha256=$digest"
}

test_local_candidate_stages_once_before_verification() {
    local case_root="$test_root/local-toctou"
    local fixtures="$case_root/fixtures" fake_bin="$case_root/fake-bin" home="$case_root/home"
    mkdir -p "$fixtures" "$home" "$fake_bin"
    make_fake_tools "$fake_bin" "$fixtures"
    local candidate="$case_root/labby"
    printf '#!/bin/sh\necho original\n' >"$candidate"; chmod 755 "$candidate"
    digest=$(shasum -a 256 "$candidate" | awk '{print $1}')
    cat >"$fake_bin/sha256sum" <<'EOF'
#!/bin/sh
/usr/bin/shasum -a 256 "$1"
case "$1" in
  "$LABBY_TEST_MUTATE_PATH") printf '#!/bin/sh\necho mutated\n' >"$1"; chmod 755 "$1" ;;
esac
EOF
    chmod 755 "$fake_bin/sha256sum"
    run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_LOCAL_BINARY="$candidate" \
        LABBY_INSTALL_LOCAL_SHA256="$digest" LABBY_INSTALL_VERSION=v2.0.0 \
        LABBY_TEST_MUTATE_PATH="$candidate" >/dev/null 2>&1
    [ "$("$home/bin/labby")" = original ] || fail "local candidate bytes changed after verification"
}

test_crash_recovery_restores_every_activation_boundary() {
    local boundary
    for boundary in binary previous receipt; do
        local case_root="$test_root/crash-$boundary"
        local fixtures="$case_root/fixtures" fake_bin="$case_root/fake-bin" home="$case_root/home"
        mkdir -p "$fixtures" "$home"
        make_release "$fixtures" v1.0.0 release-v1
        make_release "$fixtures" v2.0.0 release-v2
        make_fake_tools "$fake_bin" "$fixtures"
        run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_VERSION=v1.0.0 >/dev/null 2>&1
        cp "$home/bin/.labby-install/receipt" "$case_root/receipt.before"
        cat >"$fake_bin/mv" <<'EOF'
#!/bin/sh
destination=
for argument do destination=$argument; done
/bin/mv "$@" || exit
case "$destination:$LABBY_TEST_CRASH_BOUNDARY" in
  */bin/labby:binary|*/previous-receipt:previous|*/.labby-install/receipt:receipt)
    # This shim is invoked directly by the installer shell, so PPID is the
    # exact process whose abrupt death models the activation crash boundary.
    # Walking to its parent makes the fixture depend on wrapper/process timing.
    kill -9 "$PPID"
    ;;
esac
EOF
        chmod 755 "$fake_bin/mv"
        if run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_VERSION=v2.0.0 \
            LABBY_TEST_CRASH_BOUNDARY="$boundary" >"$case_root/out" 2>"$case_root/err"; then
            fail "crash injection at $boundary unexpectedly succeeded"
        fi
        /bin/rm -f "$fake_bin/mv" "$fixtures/releases/v2.0.0/lab-x86_64-unknown-linux-gnu.tar.gz"
        if run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_VERSION=v2.0.0 \
            >"$case_root/recovery.out" 2>"$case_root/recovery.err"; then
            fail "post-recovery unavailable release unexpectedly succeeded"
        fi
        [ "$("$home/bin/labby")" = release-v1 ] || fail "recovery at $boundary did not restore binary"
        cmp "$case_root/receipt.before" "$home/bin/.labby-install/receipt" || fail "recovery at $boundary did not restore receipt"
        [ ! -e "$home/bin/.labby-install/activation-journal" ] || fail "recovery at $boundary retained completed journal"
    done
}

test_recovery_failure_is_reported_and_journal_is_retained() {
    local case_root="$test_root/recovery-failure"
    local fixtures="$case_root/fixtures" fake_bin="$case_root/fake-bin" home="$case_root/home"
    local journal="$home/bin/.labby-install/activation-journal"
    mkdir -p "$fixtures" "$home/bin" "$journal"
    printf 'prepared\n' >"$journal/state"
    : >"$journal/old-binary.present"
    make_fake_tools "$fake_bin" "$fixtures"
    if run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_VERSION=v9 \
        >"$case_root/out" 2>"$case_root/err"; then
        fail "unrestorable activation journal unexpectedly succeeded"
    fi
    assert_contains "$case_root/err" "activation recovery FAILED"
    [ -d "$journal" ] || fail "failed recovery removed diagnostic journal"
}

test_unprepared_journal_never_changes_live_installation() {
    local case_root="$test_root/unprepared"
    local fixtures="$case_root/fixtures" fake_bin="$case_root/fake-bin" home="$case_root/home"
    local journal="$home/bin/.labby-install/activation-journal"
    mkdir -p "$fixtures" "$journal"
    printf '#!/bin/sh\necho known-good\n' >"$home/bin/labby"; chmod 755 "$home/bin/labby"
    printf 'known-good-receipt\n' >"$home/bin/.labby-install/receipt"
    printf 'partial staging' >"$journal/new-binary"
    make_fake_tools "$fake_bin" "$fixtures"
    run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_VERSION=v9 >"$case_root/out" 2>"$case_root/err" || true
    [ "$("$home/bin/labby")" = known-good ] || fail "unprepared journal changed the live binary"
    assert_contains "$home/bin/.labby-install/receipt" "known-good-receipt"
    [ ! -d "$journal" ] || fail "unprepared journal was not discarded"
}

test_activation_failure_restores_binary_and_both_receipts() {
    local case_root="$test_root/activation"
    local fixtures="$case_root/fixtures" fake_bin="$case_root/fake-bin" home="$case_root/home"
    mkdir -p "$fixtures" "$home"
    make_release "$fixtures" v1.0.0 release-v1
    make_release "$fixtures" v2.0.0 release-v2
    make_fake_tools "$fake_bin" "$fixtures"
    run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_VERSION=v1.0.0 >/dev/null 2>&1
    cp "$home/bin/.labby-install/receipt" "$case_root/receipt.before"
    cat >"$fake_bin/mv" <<'EOF'
#!/bin/sh
destination=
for argument do destination=$argument; done
case "$destination" in
    */.labby-install/receipt)
        if [ ! -f "$LABBY_TEST_FAIL_ONCE" ]; then : >"$LABBY_TEST_FAIL_ONCE"; exit 9; fi
        ;;
esac
exec /bin/mv "$@"
EOF
    chmod 755 "$fake_bin/mv"
    if run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_VERSION=v2.0.0 \
        LABBY_TEST_FAIL_RECEIPT_PATH="$home/bin/.labby-install/receipt" \
        LABBY_TEST_FAIL_ONCE="$case_root/failed" >"$case_root/out" 2>"$case_root/err"; then
        fail "injected post-activation receipt failure unexpectedly succeeded"
    fi
    [ "$("$home/bin/labby")" = release-v1 ] || fail "activation failure did not restore the prior binary"
    cmp "$case_root/receipt.before" "$home/bin/.labby-install/receipt" || fail "activation failure changed current receipt"
    [ ! -f "$home/bin/.labby-install/previous-receipt" ] || fail "activation failure created prior receipt"
}

run_installer() {
    local home=$1 fixtures=$2 fake_bin=$3
    shift 3
    env -i \
        HOME="$home" \
        PATH="$fake_bin:/usr/bin:/bin:/usr/sbin:/sbin" \
        LABBY_TEST_FIXTURES="$fixtures" \
        LABBY_INSTALL_DIR="$home/bin" \
        LABBY_INSTALL_REPO="example/labby" \
        "$@" \
        /bin/sh "$repo_root/scripts/install.sh"
}

test_checksum_mismatch_fails_closed_and_preserves_prior_binary() {
    local case_root="$test_root/checksum"
    local fixtures="$case_root/fixtures" fake_bin="$case_root/fake-bin" home="$case_root/home"
    mkdir -p "$fixtures" "$home/bin"
    make_release "$fixtures" v1.0.0 release-v1
    printf '%064d  lab-x86_64-unknown-linux-gnu.tar.gz\n' 0 \
        >"$fixtures/releases/v1.0.0/lab-x86_64-unknown-linux-gnu.tar.gz.sha256"
    printf '#!/bin/sh\necho sentinel\n' >"$home/bin/labby"
    chmod 755 "$home/bin/labby"
    make_fake_tools "$fake_bin" "$fixtures"

    if run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_VERSION=v1.0.0 \
        >"$case_root/out" 2>"$case_root/err"; then
        fail "checksum mismatch unexpectedly succeeded"
    fi

    [ "$("$home/bin/labby")" = sentinel ] || fail "checksum failure replaced prior binary"
    assert_contains "$case_root/err" "checksum verification FAILED"
}

test_latest_selects_newest_release_with_platform_asset() {
    local case_root="$test_root/latest"
    local fixtures="$case_root/fixtures" fake_bin="$case_root/fake-bin" home="$case_root/home"
    mkdir -p "$fixtures" "$home"
    make_release "$fixtures" v1.0.0 release-v1
    cat >"$fixtures/releases.json" <<'EOF'
[
  {
    "tag_name": "incus-v9",
    "name": "labby-incus.tar.gz"
  },
  {
    "tag_name": "v1.0.0",
    "name": "lab-x86_64-unknown-linux-gnu.tar.gz"
  }
]
EOF
    make_fake_tools "$fake_bin" "$fixtures"

    run_installer "$home" "$fixtures" "$fake_bin" >"$case_root/out" 2>"$case_root/err"

    [ "$("$home/bin/labby")" = release-v1 ] || fail "latest selected the wrong release"
    assert_contains "$case_root/err" "resolved latest binary release to v1.0.0"
}

test_receipt_and_offline_rollback_restore_exact_prior_binary() {
    local case_root="$test_root/rollback"
    local fixtures="$case_root/fixtures" fake_bin="$case_root/fake-bin" home="$case_root/home"
    mkdir -p "$fixtures" "$home"
    make_release "$fixtures" v1.0.0 release-v1
    make_release "$fixtures" v2.0.0 release-v2
    make_fake_tools "$fake_bin" "$fixtures"

    run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_VERSION=v1.0.0 >/dev/null 2>&1
    run_installer "$home" "$fixtures" "$fake_bin" LABBY_INSTALL_VERSION=v2.0.0 >/dev/null 2>&1
    [ "$("$home/bin/labby")" = release-v2 ] || fail "second release was not active"

    receipt="$home/bin/.labby-install/receipt"
    [ -f "$receipt" ] || fail "installer did not retain a receipt"
    [ "$(file_mode "$receipt")" = 600 ] || fail "receipt is not owner-only"
    assert_contains "$receipt" "resolved_version=v2.0.0"
    assert_contains "$receipt" "source=release"

    rm -rf "$fixtures"
    run_installer "$home" "$case_root/offline" "$fake_bin" LABBY_INSTALL_ROLLBACK=1 \
        >"$case_root/out" 2>"$case_root/err"
    [ "$("$home/bin/labby")" = release-v1 ] || fail "offline rollback did not restore prior binary"
    assert_contains "$receipt" "resolved_version=v1.0.0"
}

test_pinned_source_fallback_preserves_requested_tag() {
    local case_root="$test_root/source"
    local fixtures="$case_root/fixtures" fake_bin="$case_root/fake-bin" home="$case_root/home"
    mkdir -p "$fixtures" "$home" "$fake_bin"
    make_fake_tools "$fake_bin" "$fixtures"
    cat >"$fake_bin/cargo" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >"$LABBY_TEST_CARGO_ARGS"
root=
while [ "$#" -gt 0 ]; do
    if [ "$1" = --root ]; then root=$2; break; fi
    shift
done
mkdir -p "$root/bin"
printf '#!/bin/sh\necho source-v3\n' >"$root/bin/labby"
chmod 755 "$root/bin/labby"
EOF
    chmod 755 "$fake_bin/cargo"

    run_installer "$home" "$fixtures" "$fake_bin" \
        LABBY_INSTALL_VERSION=v3.0.0 \
        LABBY_ALLOW_SOURCE_FALLBACK=1 \
        LABBY_TEST_CARGO_ARGS="$case_root/cargo-args" >/dev/null 2>&1

    assert_contains "$case_root/cargo-args" "--tag v3.0.0"
    assert_contains "$home/bin/.labby-install/receipt" "requested_version=v3.0.0"
    assert_contains "$home/bin/.labby-install/receipt" "source=source"
}

test_latest_source_fallback_records_and_builds_resolved_revision() {
    local case_root="$test_root/source-latest"
    local fixtures="$case_root/fixtures" fake_bin="$case_root/fake-bin" home="$case_root/home"
    mkdir -p "$fixtures" "$home" "$fake_bin"
    make_fake_tools "$fake_bin" "$fixtures"
    cat >"$fake_bin/git" <<'EOF'
#!/bin/sh
echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa HEAD
EOF
    cat >"$fake_bin/cargo" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >"$LABBY_TEST_CARGO_ARGS"
root=
while [ "$#" -gt 0 ]; do
    if [ "$1" = --root ]; then root=$2; break; fi
    shift
done
mkdir -p "$root/bin"
printf '#!/bin/sh\necho source-latest\n' >"$root/bin/labby"
chmod 755 "$root/bin/labby"
EOF
    chmod 755 "$fake_bin/git" "$fake_bin/cargo"

    run_installer "$home" "$fixtures" "$fake_bin" \
        LABBY_ALLOW_SOURCE_FALLBACK=1 \
        LABBY_TEST_CARGO_ARGS="$case_root/cargo-args" >/dev/null 2>&1

    assert_contains "$case_root/cargo-args" "--rev aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    assert_contains "$home/bin/.labby-install/receipt" "resolved_version=rev:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
}

test_launchd_uses_stable_labby_home_not_installer_working_directory() {
    local case_root="$test_root/launchd"
    local fake_bin="$case_root/fake-bin" home="$case_root/home"
    local disposable="$case_root/disposable" labby_home="$home/durable-labby"
    mkdir -p "$fake_bin" "$home/.local/bin" "$disposable"
    printf '#!/bin/sh\nexit 0\n' >"$home/.local/bin/labby"
    chmod 755 "$home/.local/bin/labby"
    cat >"$fake_bin/uname" <<'EOF'
#!/bin/sh
echo Darwin
EOF
    cat >"$fake_bin/id" <<'EOF'
#!/bin/sh
echo 501
EOF
    cat >"$fake_bin/plutil" <<'EOF'
#!/bin/sh
exit 0
EOF
    cat >"$fake_bin/launchctl" <<'EOF'
#!/bin/sh
exit 0
EOF
    cat >"$fake_bin/curl" <<'EOF'
#!/bin/sh
[ "${LABBY_TEST_CURL_FAIL:-}" != 1 ] || exit 22
exit 0
EOF
    chmod 755 "$fake_bin"/*

    (
        cd "$disposable"
        env HOME="$home" PATH="$fake_bin:/usr/bin:/bin" LABBY_HOME="$labby_home" \
            bash "$repo_root/scripts/install-macos-service.sh" install >/dev/null
    )

    plist="$home/Library/LaunchAgents/ai.dinglebear.labby.plist"
    assert_contains "$plist" "<string>${labby_home}</string>"
    if grep -Fq "<string>${disposable}</string>" "$plist"; then
        fail "LaunchAgent persisted the disposable installer directory"
    fi
    assert_contains "$plist" "<key>EnvironmentVariables</key>"
    assert_contains "$plist" "<key>LABBY_HOME</key>"
}

test_launchd_rejects_relative_binary_and_state_paths() {
    local case_root="$test_root/launchd-relative" fake_bin="$test_root/launchd/fake-bin"
    mkdir -p "$case_root"
    if env HOME="$case_root" PATH="$fake_bin:/usr/bin:/bin" LABBY_SERVICE_BIN=relative/labby \
        bash "$repo_root/scripts/install-macos-service.sh" install >"$case_root/out" 2>"$case_root/err"; then
        fail "launchd accepted a relative LABBY_SERVICE_BIN"
    fi
    assert_contains "$case_root/err" "LABBY_SERVICE_BIN must be an absolute path"
    if env HOME="$case_root" PATH="$fake_bin:/usr/bin:/bin" LABBY_STATE_DIR=relative/state \
        bash "$repo_root/scripts/install-macos-service.sh" install >"$case_root/out" 2>"$case_root/err"; then
        fail "launchd accepted a relative LABBY_STATE_DIR"
    fi
    assert_contains "$case_root/err" "LABBY_STATE_DIR must be an absolute path"
}

make_fake_launchd_tools() {
    local fake_bin=$1
    mkdir -p "$fake_bin"
    cat >"$fake_bin/uname" <<'EOF'
#!/bin/sh
echo Darwin
EOF
    cat >"$fake_bin/id" <<'EOF'
#!/bin/sh
echo 501
EOF
    cat >"$fake_bin/plutil" <<'EOF'
#!/bin/sh
grep -q '<plist version="1.0">' "$2"
EOF
    cat >"$fake_bin/curl" <<'EOF'
#!/bin/sh
[ "${LABBY_TEST_CURL_FAIL:-}" != 1 ] || exit 22
exit 0
EOF
    cat >"$fake_bin/launchctl" <<'EOF'
#!/bin/sh
command=$1
printf '%s\n' "$*" >>"$LABBY_TEST_LAUNCHCTL_LOG"
if [ "${LABBY_TEST_LAUNCHCTL_FAIL:-}" = "$command" ]; then
    if [ "${LABBY_TEST_LAUNCHCTL_FAIL_ONCE:-}" != 1 ] || [ ! -e "$LABBY_TEST_LAUNCHCTL_FAIL_MARKER" ]; then
        [ "${LABBY_TEST_LAUNCHCTL_FAIL_ONCE:-}" != 1 ] || : >"$LABBY_TEST_LAUNCHCTL_FAIL_MARKER"
        exit 71
    fi
fi
case "$command" in
  bootstrap) : >"$LABBY_TEST_LAUNCHCTL_STATE" ;;
  bootout) rm -f "$LABBY_TEST_LAUNCHCTL_STATE" ;;
  print)
    [ -f "$LABBY_TEST_LAUNCHCTL_STATE" ] || exit 113
    printf '%s\n' "$*"
    ;;
  kickstart) [ -f "$LABBY_TEST_LAUNCHCTL_STATE" ] || exit 113 ;;
  *) exit 64 ;;
esac
EOF
    chmod 755 "$fake_bin"/*
}

run_macos_service() {
    local home=$1 fake_bin=$2 action=$3
    shift 3
    env HOME="$home" PATH="$fake_bin:/usr/bin:/bin" \
        LABBY_TEST_LAUNCHCTL_LOG="$home/launchctl.log" \
        LABBY_TEST_LAUNCHCTL_STATE="$home/launchctl.state" \
        LABBY_TEST_LAUNCHCTL_FAIL_MARKER="$home/launchctl.fail-marker" \
        "$@" bash "$repo_root/scripts/install-macos-service.sh" "$action"
}

test_launchd_full_lifecycle_and_repeated_install() {
    local case_root="$test_root/launchd-lifecycle"
    local fake_bin="$case_root/fake-bin" home="$case_root/home"
    mkdir -p "$home/.local/bin"
    printf '#!/bin/sh\nexit 0\n' >"$home/.local/bin/labby"
    chmod 755 "$home/.local/bin/labby"
    make_fake_launchd_tools "$fake_bin"

    run_macos_service "$home" "$fake_bin" install >/dev/null
    run_macos_service "$home" "$fake_bin" status >"$case_root/status"
    assert_contains "$case_root/status" "print gui/501/ai.dinglebear.labby"
    run_macos_service "$home" "$fake_bin" restart >/dev/null
    run_macos_service "$home" "$fake_bin" install >/dev/null
    [ "$(grep -c '^bootstrap ' "$home/launchctl.log")" = 2 ] || fail "repeated install did not bootstrap twice"
    [ "$(grep -c '^bootout ' "$home/launchctl.log")" = 1 ] || fail "repeated install did not unload exactly the loaded prior service"
    run_macos_service "$home" "$fake_bin" uninstall >/dev/null
    [ ! -e "$home/Library/LaunchAgents/ai.dinglebear.labby.plist" ] || fail "uninstall retained the plist"
    if run_macos_service "$home" "$fake_bin" status >"$case_root/out" 2>"$case_root/err"; then
        fail "status succeeded after uninstall"
    fi
}

test_launchd_plist_escapes_all_configured_xml_values() {
    local case_root="$test_root/launchd-xml"
    local fake_bin="$case_root/fake-bin" home="$case_root/home"
    local binary="$home/bin/labby&tool" labby_home="$home/state&<dir>" log_dir="$home/logs'&dir"
    mkdir -p "$(dirname "$binary")"
    printf '#!/bin/sh\nexit 0\n' >"$binary"; chmod 755 "$binary"
    make_fake_launchd_tools "$fake_bin"

    run_macos_service "$home" "$fake_bin" install \
        LABBY_SERVICE_BIN="$binary" LABBY_HOME="$labby_home" LABBY_STATE_DIR="$log_dir" \
        LABBY_SERVICE_HOST='host&<name>' LABBY_SERVICE_PORT='87&65' >/dev/null
    local plist="$home/Library/LaunchAgents/ai.dinglebear.labby.plist"
    assert_contains "$plist" 'labby&amp;tool'
    assert_contains "$plist" 'state&amp;&lt;dir&gt;'
    assert_contains "$plist" 'logs&apos;&amp;dir'
    assert_contains "$plist" 'host&amp;&lt;name&gt;'
    assert_contains "$plist" '87&amp;65'
}

test_launchd_propagates_launchctl_failures() {
    local command
    for command in bootstrap kickstart health; do
        local case_root="$test_root/launchd-fail-$command"
        local fake_bin="$case_root/fake-bin" home="$case_root/home"
        mkdir -p "$home/.local/bin"
        printf '#!/bin/sh\nexit 0\n' >"$home/.local/bin/labby"; chmod 755 "$home/.local/bin/labby"
        make_fake_launchd_tools "$fake_bin"
        run_macos_service "$home" "$fake_bin" install >/dev/null
        local plist="$home/Library/LaunchAgents/ai.dinglebear.labby.plist"
        printf '\n<!-- prior -->\n' >>"$plist"
        local before
        before=$(shasum -a 256 "$plist" | awk '{print $1}')
        local -a failure_env=(LABBY_TEST_LAUNCHCTL_FAIL="$command" LABBY_TEST_LAUNCHCTL_FAIL_ONCE=1)
        if [[ "$command" == health ]]; then
            failure_env=(LABBY_TEST_CURL_FAIL=1 LABBY_TEST_HEALTH_ATTEMPTS=1)
        fi
        if run_macos_service "$home" "$fake_bin" install "${failure_env[@]}" \
            >"$case_root/out" 2>"$case_root/err"; then
            fail "install swallowed injected $command failure"
        fi
        [ "$(shasum -a 256 "$plist" | awk '{print $1}')" = "$before" ] || \
            fail "$command failure did not restore the prior plist"
        [ -e "$home/launchctl.state" ] || fail "$command failure did not reload the prior service"
    done

    local case_root="$test_root/launchd-fail-bootout"
    local fake_bin="$case_root/fake-bin" home="$case_root/home"
    mkdir -p "$home/.local/bin"
    printf '#!/bin/sh\nexit 0\n' >"$home/.local/bin/labby"; chmod 755 "$home/.local/bin/labby"
    make_fake_launchd_tools "$fake_bin"
    run_macos_service "$home" "$fake_bin" install >/dev/null
    local plist="$home/Library/LaunchAgents/ai.dinglebear.labby.plist"
    local before
    before=$(shasum -a 256 "$plist" | awk '{print $1}')
    if run_macos_service "$home" "$fake_bin" install LABBY_TEST_LAUNCHCTL_FAIL=bootout \
        >"$case_root/out" 2>"$case_root/err"; then
        fail "install swallowed injected launchctl bootout failure"
    fi
    [ "$(shasum -a 256 "$plist" | awk '{print $1}')" = "$before" ] || \
        fail "bootout failure replaced the prior live plist"

    case_root="$test_root/launchd-fail-print"
    fake_bin="$case_root/fake-bin"; home="$case_root/home"
    mkdir -p "$home/.local/bin"
    printf '#!/bin/sh\nexit 0\n' >"$home/.local/bin/labby"; chmod 755 "$home/.local/bin/labby"
    make_fake_launchd_tools "$fake_bin"
    run_macos_service "$home" "$fake_bin" install >/dev/null
    if run_macos_service "$home" "$fake_bin" restart LABBY_TEST_LAUNCHCTL_FAIL=print \
        >"$case_root/out" 2>"$case_root/err"; then
        fail "restart treated injected launchctl print failure as absence"
    fi
    [ "$(grep -c '^bootstrap ' "$home/launchctl.log")" = 1 ] || \
        fail "restart bootstrapped after a non-NotFound launchctl print failure"

    if run_macos_service "$home" "$fake_bin" status LABBY_TEST_LAUNCHCTL_FAIL=print \
        >"$case_root/status-out" 2>"$case_root/status-err"; then
        fail "status swallowed injected launchctl print failure"
    fi
    if grep -Fq 'is not loaded' "$case_root/status-err"; then
        fail "status misreported a non-NotFound launchctl failure as absence"
    fi
}

test_root_installer_is_self_contained_when_piped_from_arbitrary_cwd
test_latest_api_failure_never_uses_mutable_latest_download
test_release_failure_matrix_preserves_existing_binary
test_checksum_ignores_sidecar_subject_and_hashes_requested_archive
test_cached_artifact_is_rehashed_before_reuse
test_local_candidate_requires_and_records_exact_digest
test_local_candidate_stages_once_before_verification
test_activation_failure_restores_binary_and_both_receipts
test_crash_recovery_restores_every_activation_boundary
test_recovery_failure_is_reported_and_journal_is_retained
test_unprepared_journal_never_changes_live_installation
test_launchd_uses_stable_labby_home_not_installer_working_directory
test_launchd_rejects_relative_binary_and_state_paths
test_launchd_full_lifecycle_and_repeated_install
test_launchd_plist_escapes_all_configured_xml_values
test_launchd_propagates_launchctl_failures
test_checksum_mismatch_fails_closed_and_preserves_prior_binary
test_latest_selects_newest_release_with_platform_asset
test_receipt_and_offline_rollback_restore_exact_prior_binary
test_pinned_source_fallback_preserves_requested_tag
test_latest_source_fallback_records_and_builds_resolved_revision
echo "installer behavioral tests passed"
