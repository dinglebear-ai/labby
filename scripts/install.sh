#!/bin/sh
# Install labby — the Lab homelab control plane binary.
#
# Obtain the standalone script from the canonical repository over HTTPS, or
# independently verify its release checksum and provenance before running it.
# The reviewed script carries pinned verifier hashes; no account is needed for
# releases publishing public provenance bundles.
#
# Downloads the latest GitHub release archive for this platform, verifies its
# SHA-256, and installs the binary to ~/.local/bin/labby. When explicitly
# enabled with LABBY_ALLOW_SOURCE_FALLBACK=1, a release failure falls back to
# `cargo install --git` if a Rust toolchain is available.
#
# The shell layer owns verified binary bootstrap. Once activation succeeds it
# hands control to the Rust-owned `labby setup` flow so a single command can
# finish server/client onboarding without duplicating product logic in shell.
#
# Environment overrides:
#   LABBY_INSTALL_DIR     install directory       (default: ~/.local/bin)
#   LABBY_INSTALL_REPO    owner/repo to fetch     (default: dinglebear-ai/labby)
#   LABBY_INSTALL_VERSION release tag, e.g. v0.22.2 (default: latest)
#   LABBY_ALLOW_SOURCE_FALLBACK allow cargo fallback after release failure (default: 0)
#   LABBY_INSTALL_RECOVER_ONLY settle a pending activation offline without installing (default: 0)
#   LABBY_INSTALL_ROLLBACK restore the previous verified binary offline (default: 0)
#   LABBY_INSTALL_LOCAL_BINARY install an exact local candidate (requires SHA-256)
#   LABBY_INSTALL_LOCAL_SHA256 expected digest for LABBY_INSTALL_LOCAL_BINARY
#   LABBY_INSTALL_LOCAL_COMPANIONS version-matched Tailcat directory for a local candidate
#   LABBY_INSTALL_NO_SETUP skip first-run setup after install (default: 0)
#   LABBY_SETUP_ROLE      noninteractive role: server|client
#   LABBY_SETUP_DEPLOYMENT server backend: native|incus
#   LABBY_SETUP_HOST / LABBY_SETUP_PORT server listen address / port
#   LABBY_SETUP_SERVER_URL client server URL
#   LABBY_SETUP_PUBLIC_URL public browser/OAuth URL
#   LABBY_SETUP_AUTH      bearer|oauth|both
#   LABBY_SETUP_OAUTH     none|google|authelia
#   LABBY_SETUP_DESKTOP   1 install desktop, 0 skip it
#   LABBY_SETUP_NO_BROWSER 1 avoid opening a browser

set -eu

REPO="${LABBY_INSTALL_REPO:-dinglebear-ai/labby}"
INSTALL_DIR="${LABBY_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${LABBY_INSTALL_VERSION:-latest}"
ALLOW_SOURCE_FALLBACK="${LABBY_ALLOW_SOURCE_FALLBACK:-0}"
ROLLBACK="${LABBY_INSTALL_ROLLBACK:-0}"
RECOVER_ONLY="${LABBY_INSTALL_RECOVER_ONLY:-0}"
LOCAL_BINARY="${LABBY_INSTALL_LOCAL_BINARY:-}"
LOCAL_SHA256="${LABBY_INSTALL_LOCAL_SHA256:-}"
LOCAL_COMPANIONS="${LABBY_INSTALL_LOCAL_COMPANIONS:-}"
NO_SETUP="${LABBY_INSTALL_NO_SETUP:-${LABBY_SKIP_SETUP:-0}}"
SETUP_ROLE="${LABBY_SETUP_ROLE:-}"
SETUP_DEPLOYMENT="${LABBY_SETUP_DEPLOYMENT:-}"
SETUP_HOST="${LABBY_SETUP_HOST:-}"
SETUP_PORT="${LABBY_SETUP_PORT:-}"
SETUP_SERVER_URL="${LABBY_SETUP_SERVER_URL:-}"
SETUP_PUBLIC_URL="${LABBY_SETUP_PUBLIC_URL:-}"
SETUP_AUTH="${LABBY_SETUP_AUTH:-}"
SETUP_OAUTH="${LABBY_SETUP_OAUTH:-}"
SETUP_DESKTOP="${LABBY_SETUP_DESKTOP:-}"
SETUP_NO_BROWSER="${LABBY_SETUP_NO_BROWSER:-0}"
INSTALL_METADATA_DIR="$INSTALL_DIR/.labby-install"
ARTIFACTS_DIR="$INSTALL_METADATA_DIR/artifacts"
TRANSACTION_LOCK="$INSTALL_METADATA_DIR/transaction-lock"
RECEIPT_PATH="$INSTALL_METADATA_DIR/receipt"
PREVIOUS_RECEIPT_PATH="$INSTALL_METADATA_DIR/previous-receipt"
ACTIVATION_JOURNAL="$INSTALL_METADATA_DIR/activation-journal"
TMP_DIRS=""
CREATED_TMP_DIR=""

cleanup() {
    for dir in $TMP_DIRS; do
        rm -rf "$dir"
    done
    release_transaction_lock
}
trap cleanup EXIT

make_tmp_dir() {
    CREATED_TMP_DIR="$(mktemp -d)"
    TMP_DIRS="${TMP_DIRS} ${CREATED_TMP_DIR}"
}

say() { printf '%s\n' "$*" >&2; }
fail() { say "install.sh: $*"; exit 1; }

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "$2"
}

# BEGIN GENERATED GITHUB VERIFIER BOOTSTRAP
# shellcheck shell=sh
# Generated trust code; edit this template and reviewed bootstrap-pins.json.
# Source this helper and call ensure_github_verifier <private temporary directory>.
github_verifier_version_ok() {
    "$1" --version 2>/dev/null | awk '
      NR == 1 && $1 == "gh" && $2 == "version" {
        split($3,v,".");
        if (v[1] ~ /^[0-9]+$/ && v[2] ~ /^[0-9]+$/ && v[3] ~ /^[0-9]+$/ &&
            (v[1] > 2 || (v[1] == 2 && (v[2] > 102 || (v[2] == 102 && v[3] >= 0))))) ok=1
      } END { exit !ok }'
}
ensure_github_verifier() {
    verifier_tmp=$(cd "$1" && pwd) || return 1
    GH_VERIFIER=${GH_VERIFIER:-$(command -v gh 2>/dev/null || true)}
    if [ -n "$GH_VERIFIER" ] && github_verifier_version_ok "$GH_VERIFIER" && "$GH_VERIFIER" attestation verify --help >/dev/null 2>&1; then
        case "$GH_VERIFIER" in /*) ;; *) GH_VERIFIER="$(cd "$(dirname "$GH_VERIFIER")" && pwd)/$(basename "$GH_VERIFIER")" ;; esac
        return 0
    fi
    verifier_platform="$(uname -s)/$(uname -m)"
    case "$verifier_platform" in
        Linux/x86_64) verifier_asset="gh_2.102.0_linux_amd64.tar.gz"; verifier_digest="bb766f710eef8ede859c18578c72c327597cd4c8a85b06001b1f3843c6019386"; verifier_member="gh_2.102.0_linux_amd64/bin/gh" ;;
        Linux/aarch64|Linux/arm64) verifier_asset="gh_2.102.0_linux_arm64.tar.gz"; verifier_digest="7862c86c72f43df3a2d93ddde6f473285b4e2af61b494849846827e513ef6484"; verifier_member="gh_2.102.0_linux_arm64/bin/gh" ;;
        Darwin/arm64) verifier_asset="gh_2.102.0_macOS_arm64.zip"; verifier_digest="da922c20d1792e5b2cbf375593d7a658acf034c12c84e007e71c76ef959c337e"; verifier_member="gh_2.102.0_macOS_arm64/bin/gh" ;;
        MINGW*/x86_64|MSYS*/x86_64|CYGWIN*/x86_64) verifier_asset="gh_2.102.0_windows_amd64.zip"; verifier_digest="ae64e556ecc240b200f7eba60d550e4bb60d78e860e69dd88c449405b86067f4"; verifier_member="bin/gh.exe" ;;
        MINGW*/aarch64|MINGW*/arm64|MSYS*/aarch64|MSYS*/arm64) verifier_asset="gh_2.102.0_windows_arm64.zip"; verifier_digest="5dcf12aa8525eabd0c46ec414f323ab6cf65229fc2cd46543cc705001bbaf223"; verifier_member="bin/gh.exe" ;;
        *) printf '%s\n' 'No pinned GitHub verifier is available for this platform.' >&2; return 1 ;;
    esac
    verifier_archive="$verifier_tmp/$verifier_asset"
    printf '%s\n' 'Bootstrapping pinned GitHub verifier 2.102.0 (no account required).' >&2
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 --max-redirs 5 -fSL --connect-timeout 10 --max-time 120 --max-filesize 25000000 --retry 2 -o "$verifier_archive" "https://github.com/cli/cli/releases/download/v2.102.0/$verifier_asset" || return 1
    if command -v sha256sum >/dev/null 2>&1; then
        verifier_actual=$(sha256sum "$verifier_archive" | awk '{print $1}')
    elif command -v shasum >/dev/null 2>&1; then
        verifier_actual=$(shasum -a 256 "$verifier_archive" | awk '{print $1}')
    else
        printf '%s\n' 'A SHA-256 tool is required for the verifier bootstrap.' >&2; return 1
    fi
    [ "$verifier_actual" = "$verifier_digest" ] || { printf '%s\n' 'Pinned verifier checksum FAILED; downloaded code was not executed.' >&2; return 1; }
    case "$verifier_asset" in
        *.tar.gz) tar -xzf "$verifier_archive" -C "$verifier_tmp" "$verifier_member" || return 1 ;;
        *.zip) command -v unzip >/dev/null 2>&1 || { printf '%s\n' 'unzip is required for the macOS verifier bootstrap.' >&2; return 1; }; unzip -q "$verifier_archive" "$verifier_member" -d "$verifier_tmp" || return 1 ;;
    esac
    GH_VERIFIER="$verifier_tmp/$verifier_member"
    [ -f "$GH_VERIFIER" ] && [ ! -L "$GH_VERIFIER" ] || return 1
    chmod 700 "$GH_VERIFIER" || return 1
    if ! github_verifier_version_ok "$GH_VERIFIER" || ! "$GH_VERIFIER" attestation verify --help >/dev/null 2>&1; then
        printf '%s\n' 'Pinned verifier does not support the required policy.' >&2
        return 1
    fi
}
# END GENERATED GITHUB VERIFIER BOOTSTRAP

# Require the verifier before downloading release bytes. Published attestation
# bundles permit verification without a GitHub account; older releases use the
# authenticated attestation API. Both paths enforce the same signer policy.
require_release_prerequisites() {
    require_command curl "curl is required for release installation"
    require_command tar "tar is required to unpack the Labby release archive"
    if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
        fail "sha256sum or shasum is required to verify the Labby release checksum"
    fi
    make_tmp_dir
    ensure_github_verifier "$CREATED_TMP_DIR" || fail "A verified GitHub CLI 2.102.0 or newer is required; verifier bootstrap failed"
}

print_banner() {
    say ""
    say "  _          _     _"
    say " | |    __ _| |__ | |__  _   _"
    say " | |   / _\` | '_ \\| '_ \\| | | |"
    say " | |__| (_| | |_) | |_) | |_| |"
    say " |_____\\__,_|_.__/|_.__/ \\__, |"
    say "                         |___/"
    say ""
    say "  One setup. Every interface."
    say ""
}

durability_barrier() { sync || fail "cannot durably flush installer transaction"; }

release_transaction_lock() {
    [ -d "$TRANSACTION_LOCK" ] || return 0
    [ "$(cat "$TRANSACTION_LOCK/pid" 2>/dev/null || true)" = "$$" ] || return 0
    rm -rf "$TRANSACTION_LOCK"
}

acquire_transaction_lock() {
    mkdir -p "$INSTALL_METADATA_DIR"
    if ! mkdir "$TRANSACTION_LOCK" 2>/dev/null; then
        owner=$(cat "$TRANSACTION_LOCK/pid" 2>/dev/null || true)
        case "$owner" in *[!0-9]*|'') owner= ;; esac
        [ -n "$owner" ] || fail "another Labby installation is starting"
        if [ -n "$owner" ] && kill -0 "$owner" 2>/dev/null; then
            fail "another Labby installation is running (pid $owner)"
        fi
        stale="$TRANSACTION_LOCK.stale.$$"
        mv "$TRANSACTION_LOCK" "$stale" 2>/dev/null || fail "another Labby installation is starting"
        rm -rf "$stale"
        mkdir "$TRANSACTION_LOCK" 2>/dev/null || fail "another Labby installation is starting"
    fi
    printf '%s\n' "$$" >"$TRANSACTION_LOCK/pid"
    durability_barrier
}

target_triple() {
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os" in
        Linux)
            case "$arch" in
                x86_64) echo "x86_64-unknown-linux-gnu" ;;
                aarch64|arm64) echo "aarch64-unknown-linux-gnu" ;;
                *) fail "unsupported platform ${os}/${arch}; supported: Linux/x86_64, Linux/arm64" ;;
            esac
            ;;
        Darwin)
            case "$arch" in
                arm64) echo "aarch64-apple-darwin" ;;
                *) fail "unsupported platform ${os}/${arch}; supported: macOS/arm64" ;;
            esac
            ;;
        *) fail "unsupported platform ${os}/${arch}; supported: Linux/x86_64, Linux/arm64 and macOS/arm64" ;;
    esac
}

sha256_check() {
    # $1 = file, $2 = expected-checksum file (file is "<hex>  <name>" format)
    expected="$(awk 'NR == 1 { print $1 }' "$2")"
    [ "${#expected}" -eq 64 ] || return 1
    case "$expected" in *[!0-9A-Fa-f]*) return 1 ;; esac
    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$1" | awk '{print $1}')"
        [ "$expected" = "$actual" ]
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "$1" | awk '{print $1}')"
        [ "$expected" = "$actual" ]
    else
        fail "no sha256sum/shasum found and checksum verification is required"
    fi
}

verify_release_provenance() {
    artifact=$1
    resolved=$2
    bundle=${3:-}
    [ -n "${GH_VERIFIER:-}" ] || fail "GitHub verifier was not prepared"
    # Pin the trust root: GH_HOST or a gh config default must not redirect
    # attestation verification to another host.
    set -- "$artifact"
    if [ -n "$bundle" ]; then
        set -- "$@" --bundle "$bundle"
    fi
    if [ -n "$bundle" ]; then
        make_tmp_dir
        GH_TOKEN='' GITHUB_TOKEN='' GH_ENTERPRISE_TOKEN='' GITHUB_ENTERPRISE_TOKEN='' GH_HOST=github.com GH_CONFIG_DIR="$CREATED_TMP_DIR" \
            "$GH_VERIFIER" attestation verify "$@" \
        --hostname github.com \
        --repo "$REPO" \
        --signer-workflow "$REPO/.github/workflows/release.yml" \
        --source-ref "refs/tags/$resolved" \
        --deny-self-hosted-runners >/dev/null \
        || fail "GitHub provenance verification FAILED for $asset"
    else
        "$GH_VERIFIER" attestation verify "$@" --hostname github.com --repo "$REPO" --signer-workflow "$REPO/.github/workflows/release.yml" --source-ref "refs/tags/$resolved" --deny-self-hosted-runners >/dev/null || fail "GitHub provenance verification FAILED for $asset"
    fi
    say "GitHub provenance verified"
}

latest_release_with_asset() {
    # GitHub's "latest" release may point at non-binary artifacts such as the
    # Incus image release. Pick the newest release that actually contains the
    # platform binary archive we are about to download.
    # $1 = asset name
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 --max-redirs 5 -fsSL --connect-timeout 10 --max-time 300 --retry 3 "https://api.github.com/repos/${REPO}/releases?per_page=20" |
        awk -v asset="$1" '
            function capture_if_match() {
                if (resolved == "" && tag != "" && found) {
                    resolved = tag
                }
            }
            /"tag_name":[[:space:]]*"/ {
                capture_if_match()
                tag = $0
                sub(/^.*"tag_name":[[:space:]]*"/, "", tag)
                sub(/".*$/, "", tag)
                found = 0
            }
            /"name":[[:space:]]*"/ {
                name = $0
                sub(/^.*"name":[[:space:]]*"/, "", name)
                sub(/".*$/, "", name)
                if (name == asset) {
                    found = 1
                }
            }
            END {
                capture_if_match()
                if (resolved != "") {
                    print resolved
                }
            }
        '
}

binary_sha256() (
    if command -v sha256sum >/dev/null 2>&1; then
        checksum_output=$(sha256sum "$1") || exit 1
    elif command -v shasum >/dev/null 2>&1; then
        checksum_output=$(shasum -a 256 "$1") || exit 1
    else
        fail "no sha256sum/shasum found; artifact identity cannot be recorded"
    fi
    printf '%s\n' "$checksum_output" | awk '{print $1}'
)

# Pin the complete companion inventory when caching the verified release. This
# receipt is outside the bundle, so rollback can detect modified cached files.
companion_inventory() (
    cd "$1" || exit 1
    [ -z "$(find . -type l -print -quit)" ] || exit 1
    # Reject names the line-oriented receipt cannot represent before hashing.
    find . -exec sh -c '
        for companion_path do
            case "$companion_path" in *"
"*) exit 1 ;; esac
        done
    ' sh {} + || exit 1
    find . -type d -print | LC_ALL=C sort
    find . -type f -print | LC_ALL=C sort | while IFS= read -r companion_file; do
        companion_digest=$(binary_sha256 "$companion_file") || exit 1
        printf '%s %s\n' "$companion_digest" "$companion_file"
    done
)

verify_cached_companions() {
    [ -f "$1/tailcat.inventory" ] && [ ! -L "$1/tailcat.inventory" ] ||
        fail "cached Tailcat inventory receipt is unavailable; reinstall the verified release"
    companion_check=$(mktemp "$1/.inventory-check.XXXXXX")
    companion_inventory "$1/tailcat" >"$companion_check" || fail "invalid cached Tailcat inventory"
    cmp "$1/tailcat.inventory" "$companion_check" >/dev/null || {
        rm -f "$companion_check"
        fail "cached Tailcat companions differ from their verified inventory"
    }
    rm -f "$companion_check"
}

receipt_value() {
    # Receipt values are deliberately a restricted, non-executable format.
    case "$1" in
        *[!A-Za-z0-9._+:/@-]*) fail "unsafe receipt value: $1" ;;
        *) printf '%s' "$1" ;;
    esac
}

write_receipt() {
    # $1 = destination, $2 = source, $3 = requested version,
    # $4 = resolved version, $5 = binary digest.
    destination=$1
    receipt_source=$(receipt_value "$2")
    receipt_requested=$(receipt_value "$3")
    receipt_resolved=$(receipt_value "$4")
    receipt_digest=$(receipt_value "$5")
    receipt_installed_at=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
    receipt_tmp=$(mktemp "$INSTALL_METADATA_DIR/.receipt.XXXXXX")
    chmod 600 "$receipt_tmp"
    {
        printf 'format=1\n'
        printf 'source=%s\n' "$receipt_source"
        printf 'requested_version=%s\n' "$receipt_requested"
        printf 'resolved_version=%s\n' "$receipt_resolved"
        printf 'sha256=%s\n' "$receipt_digest"
        printf 'artifact_sha256=%s\n' "$artifact_identity"
        printf 'installed_at=%s\n' "$receipt_installed_at"
    } >"$receipt_tmp"
    mv -f "$receipt_tmp" "$destination"
}

receipt_field() {
    key=$1
    receipt=$2
    sed -n "s/^${key}=//p" "$receipt" | head -n 1
}

recover_activation() {
    [ -d "$ACTIVATION_JOURNAL" ] || return 0
    if [ ! -f "$ACTIVATION_JOURNAL/state" ]; then
        rm -rf "$ACTIVATION_JOURNAL" || {
            say "activation recovery FAILED removing unprepared journal"
            return 1
        }
        return 0
    fi
    say "recovering interrupted installation transaction"
    recovery_failed=0
    if [ -f "$ACTIVATION_JOURNAL/companions-managed" ]; then
        rm -rf "$INSTALL_DIR/tailcat" || recovery_failed=1
        if [ -f "$ACTIVATION_JOURNAL/old-tailcat.present" ]; then
            cp -Rp "$ACTIVATION_JOURNAL/old-tailcat" "$INSTALL_DIR/tailcat" || recovery_failed=1
        fi
    fi
    for recovery_name in binary receipt previous; do
        case "$recovery_name" in
            binary) recovery_target="$INSTALL_DIR/labby"; recovery_mode=755 ;;
            receipt) recovery_target="$RECEIPT_PATH"; recovery_mode=600 ;;
            previous) recovery_target="$PREVIOUS_RECEIPT_PATH"; recovery_mode=600 ;;
        esac
        if [ -f "$ACTIVATION_JOURNAL/old-${recovery_name}.present" ]; then
            if [ ! -f "$ACTIVATION_JOURNAL/old-${recovery_name}" ] ||
                ! install -m "$recovery_mode" "$ACTIVATION_JOURNAL/old-${recovery_name}" "$recovery_target"; then
                say "activation recovery FAILED restoring $recovery_name"
                recovery_failed=1
            fi
        elif ! rm -f "$recovery_target"; then
            say "activation recovery FAILED removing new $recovery_name"
            recovery_failed=1
        fi
    done
    [ "$recovery_failed" -eq 0 ] || return 1
    # Retire the restore marker before deleting backups. Cancellation during
    # cleanup must not make a later recovery interpret missing backups as files
    # that were absent from the committed installation.
    rm -f "$ACTIVATION_JOURNAL/state" || {
        say "activation recovery FAILED retiring completed journal"
        return 1
    }
    rm -rf "$ACTIVATION_JOURNAL" || {
        say "activation recovery FAILED removing completed journal"
        return 1
    }
    durability_barrier
    say "interrupted installation transaction restored"
}

write_activation_state() {
    state_tmp="$ACTIVATION_JOURNAL/.state.$$"
    printf '%s\n' "$1" >"$state_tmp"
    chmod 600 "$state_tmp"
    mv -f "$state_tmp" "$ACTIVATION_JOURNAL/state"
    durability_barrier
}

receipt_digest() {
    [ -f "$1" ] || return 0
    identity=$(receipt_field artifact_sha256 "$1")
    [ -n "$identity" ] || identity=$(receipt_field sha256 "$1")
    printf '%s\n' "$identity"
}

prune_unreferenced_artifacts() {
    current=$(receipt_digest "$RECEIPT_PATH")
    previous=$(receipt_digest "$PREVIOUS_RECEIPT_PATH")
    [ -d "$ARTIFACTS_DIR" ] || return 0
    for directory in "$ARTIFACTS_DIR"/*; do
        [ -d "$directory" ] || continue
        digest=${directory##*/}
        [ "$digest" = "$current" ] || [ "$digest" = "$previous" ] || rm -rf "$directory"
    done
    durability_barrier
}

install_binary_atomic() {
    # $1 = source binary, $2 = provenance, $3 = resolved version.
    source_binary=$1
    install_source=$2
    resolved_version=$3
    source_companions=${4:-}
    mkdir -p "$INSTALL_DIR" "$ARTIFACTS_DIR"
    chmod 700 "$INSTALL_METADATA_DIR" "$ARTIFACTS_DIR"
    recover_activation || fail "activation recovery FAILED; journal retained at $ACTIVATION_JOURNAL"
    digest=$(binary_sha256 "$source_binary")
    artifact_identity=${5:-$digest}
    [ "${#artifact_identity}" -eq 64 ] || fail "invalid artifact identity"
    case "$artifact_identity" in *[!0-9a-f]*) fail "invalid artifact identity" ;; esac
    artifact_dir="$ARTIFACTS_DIR/$artifact_identity"
    artifact="$artifact_dir/labby"
    if [ ! -f "$artifact" ]; then
        mkdir -p "$artifact_dir"
        chmod 700 "$artifact_dir"
        artifact_tmp=$(mktemp "$artifact_dir/.labby.XXXXXX")
        install -m 755 "$source_binary" "$artifact_tmp"
        mv -f "$artifact_tmp" "$artifact"
        durability_barrier
    elif [ "$(binary_sha256 "$artifact")" != "$digest" ]; then
        fail "cached artifact digest does not match its content: $digest"
    fi
    if [ -n "$source_companions" ]; then
        [ -d "$source_companions" ] && [ ! -L "$source_companions" ] && [ -f "$source_companions/manifest.json" ] && [ ! -L "$source_companions/manifest.json" ] || fail "Tailcat companions have no regular manifest"
        [ -z "$(find "$source_companions" -type l -print -quit)" ] || fail "Tailcat companions contain symlinks"
        if [ -d "$artifact_dir/tailcat" ]; then
            verify_cached_companions "$artifact_dir"
            cmp "$source_companions/manifest.json" "$artifact_dir/tailcat/manifest.json" >/dev/null || fail "binary artifact already has different Tailcat companions"
        else
            companion_tmp=$(mktemp -d "$artifact_dir/.tailcat.XXXXXX")
            cp -Rp "$source_companions/." "$companion_tmp/"
            companion_inventory_tmp=$(mktemp "$artifact_dir/.tailcat-inventory.XXXXXX")
            companion_inventory "$companion_tmp" >"$companion_inventory_tmp" || fail "invalid Tailcat inventory"
            chmod 600 "$companion_inventory_tmp"
            durability_barrier
            # Publish the complete receipt before the directory that marks this
            # cache usable. A stopped install can then retry the same artifact;
            # an existing companion directory always has its pinned inventory.
            mv "$companion_inventory_tmp" "$artifact_dir/tailcat.inventory"
            durability_barrier
            mv "$companion_tmp" "$artifact_dir/tailcat"
            durability_barrier
        fi
    elif [ -d "$artifact_dir/tailcat" ]; then
        fail "binary artifact already has Tailcat companions; refusing unmatched activation"
    fi
    activation_dir="$ACTIVATION_JOURNAL"
    mkdir "$activation_dir"
    chmod 700 "$activation_dir"
    install -m 755 "$artifact" "$activation_dir/new-binary"
    write_receipt "$activation_dir/receipt" "$install_source" "$VERSION" "$resolved_version" "$digest"
    if [ -f "$RECEIPT_PATH" ]; then
        cp "$RECEIPT_PATH" "$activation_dir/new-previous"
        chmod 600 "$activation_dir/new-previous"
    fi
    if [ -f "$INSTALL_DIR/labby" ]; then cp "$INSTALL_DIR/labby" "$activation_dir/old-binary"; : >"$activation_dir/old-binary.present"; fi
    if [ -f "$RECEIPT_PATH" ]; then cp "$RECEIPT_PATH" "$activation_dir/old-receipt"; : >"$activation_dir/old-receipt.present"; fi
    if [ -f "$PREVIOUS_RECEIPT_PATH" ]; then cp "$PREVIOUS_RECEIPT_PATH" "$activation_dir/old-previous"; : >"$activation_dir/old-previous.present"; fi
    : >"$activation_dir/companions-managed"
    if [ -e "$INSTALL_DIR/tailcat" ]; then
        [ -d "$INSTALL_DIR/tailcat" ] && [ ! -L "$INSTALL_DIR/tailcat" ] || fail "installed Tailcat companions must be a regular directory"
        cp -Rp "$INSTALL_DIR/tailcat" "$activation_dir/old-tailcat"
        : >"$activation_dir/old-tailcat.present"
    fi
    if [ -n "$source_companions" ]; then cp -Rp "$artifact_dir/tailcat" "$activation_dir/new-tailcat"; fi
    write_activation_state prepared

    if ! (
        mv -f "$activation_dir/new-binary" "$INSTALL_DIR/labby" &&
        write_activation_state binary-activated &&
        rm -rf "$INSTALL_DIR/tailcat" &&
        { [ ! -d "$activation_dir/new-tailcat" ] || mv "$activation_dir/new-tailcat" "$INSTALL_DIR/tailcat"; } &&
        write_activation_state companions-activated &&
        { [ ! -f "$activation_dir/new-previous" ] || mv -f "$activation_dir/new-previous" "$PREVIOUS_RECEIPT_PATH"; } &&
        write_activation_state previous-receipt-activated &&
        mv -f "$activation_dir/receipt" "$RECEIPT_PATH"
    ); then
        say "activation failed; restoring the complete prior installation transaction"
        recover_activation || fail "activation recovery FAILED; journal retained at $ACTIVATION_JOURNAL"
        return 1
    fi
    write_activation_state receipt-activated
    rm -f "$ACTIVATION_JOURNAL/state" || fail "cannot retire activation journal; backups retained at $ACTIVATION_JOURNAL"
    durability_barrier
    rm -rf "$ACTIVATION_JOURNAL"
    durability_barrier
    prune_unreferenced_artifacts
}

install_local_binary() {
    [ -f "$LOCAL_BINARY" ] || fail "LABBY_INSTALL_LOCAL_BINARY is not a regular file"
    [ "${#LOCAL_SHA256}" -eq 64 ] || fail "LABBY_INSTALL_LOCAL_SHA256 must be a 64-character SHA-256 digest"
    case "$LOCAL_SHA256" in *[!0-9a-f]*) fail "LABBY_INSTALL_LOCAL_SHA256 must be lowercase hexadecimal" ;; esac
    make_tmp_dir
    local_staged="$CREATED_TMP_DIR/labby"
    install -m 755 "$LOCAL_BINARY" "$local_staged"
    local_actual=$(binary_sha256 "$local_staged")
    [ "$local_actual" = "$LOCAL_SHA256" ] || fail "local candidate checksum verification FAILED"
    install_binary_atomic "$local_staged" local "$VERSION" "$LOCAL_COMPANIONS"
}

rollback_offline() {
    [ -f "$PREVIOUS_RECEIPT_PATH" ] || fail "no previous verified installation is available for offline rollback"
    prior_digest=$(receipt_field sha256 "$PREVIOUS_RECEIPT_PATH")
    prior_source=$(receipt_field source "$PREVIOUS_RECEIPT_PATH")
    prior_requested=$(receipt_field requested_version "$PREVIOUS_RECEIPT_PATH")
    prior_resolved=$(receipt_field resolved_version "$PREVIOUS_RECEIPT_PATH")
    [ "${#prior_digest}" -eq 64 ] || fail "previous install receipt has an invalid artifact digest"
    case "$prior_digest" in *[!0-9a-f]*) fail "previous install receipt has an invalid artifact digest" ;; esac
    prior_identity=$(receipt_digest "$PREVIOUS_RECEIPT_PATH")
    [ "${#prior_identity}" -eq 64 ] || fail "invalid previous artifact identity"
    case "$prior_identity" in *[!0-9a-f]*) fail "invalid previous artifact identity" ;; esac
    prior_artifact="$ARTIFACTS_DIR/$prior_identity/labby"
    [ -f "$prior_artifact" ] || fail "previous verified artifact is unavailable: $prior_digest"
    actual_digest=$(binary_sha256 "$prior_artifact")
    [ "$actual_digest" = "$prior_digest" ] || fail "previous artifact digest does not match its receipt"

    VERSION=$prior_requested
    prior_companions=
    [ ! -d "$ARTIFACTS_DIR/$prior_identity/tailcat" ] || prior_companions="$ARTIFACTS_DIR/$prior_identity/tailcat"
    install_binary_atomic "$prior_artifact" "$prior_source" "$prior_resolved" "$prior_companions" "$prior_identity"
    say "restored verified installation ${prior_resolved} (${prior_digest}) without network access"
}

install_from_release() {
    require_release_prerequisites
    triple="$(target_triple)" || return 1
    asset="lab-${triple}.tar.gz"
    if [ "$VERSION" = "latest" ]; then
        resolved_version="$(latest_release_with_asset "$asset" || true)"
        if [ -n "$resolved_version" ]; then
            say "resolved latest binary release to ${resolved_version}"
            base="https://github.com/${REPO}/releases/download/${resolved_version}"
        else
            say "could not resolve an immutable latest release containing $asset"
            return 1
        fi
    else
        base="https://github.com/${REPO}/releases/download/${VERSION}"
    fi

    make_tmp_dir
    tmp="$CREATED_TMP_DIR"

    # A bundle is untrusted input until gh validates its signature, artifact
    # digest, repository, workflow and source ref. Never fall back after a
    # published bundle fails verification.
    provenance_bundle=
    bundle_status=0
    bundle_http=$(curl --proto '=https' --proto-redir '=https' --tlsv1.2 --max-redirs 5 -fsSL --connect-timeout 10 --max-time 60 --retry 3 --max-filesize 5000000 -w '%{http_code}' -o "$tmp/$asset.sigstore.jsonl" "${base}/${asset}.sigstore.jsonl") || bundle_status=$?
    case "$bundle_status:$bundle_http" in
        0:200)
            [ -s "$tmp/$asset.sigstore.jsonl" ] || fail "empty release provenance bundle for $asset"
            provenance_bundle="$tmp/$asset.sigstore.jsonl"
            ;;
        # macOS curl can report HTTP errors as CURLE_RECV_ERROR (56).
        # Only a definitive 404 permits authenticated legacy verification.
        22:404|56:404)
            "$GH_VERIFIER" auth status --hostname github.com >/dev/null 2>&1 ||
                fail "This older release has no public provenance bundle; GitHub CLI must be authenticated to fetch Labby release attestations"
            ;;
        *) fail "release provenance bundle could not be retrieved; refusing authenticated fallback" ;;
    esac

    say "downloading ${base}/${asset} ..."
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 --max-redirs 5 -fsSL --connect-timeout 10 --max-time 300 --retry 3 -o "$tmp/$asset" "${base}/${asset}" || return 1
    if curl --proto '=https' --proto-redir '=https' --tlsv1.2 --max-redirs 5 -fsSL --connect-timeout 10 --max-time 300 --retry 3 -o "$tmp/$asset.sha256" "${base}/${asset}.sha256"; then
        sha256_check "$tmp/$asset" "$tmp/$asset.sha256" \
            || fail "checksum verification FAILED for $asset — aborting"
        say "sha256 verified"
    else
        fail "no .sha256 asset published for $asset; release installs require checksum verification"
    fi

    verify_release_provenance "$tmp/$asset" "${resolved_version:-$VERSION}" "$provenance_bundle"

    tar -xzf "$tmp/$asset" -C "$tmp"
    bin="$tmp/labby"
    [ -f "$bin" ] && [ ! -L "$bin" ] || fail "archive $asset did not contain a 'labby' binary"

    companions=
    if [ -d "$tmp/tailcat" ]; then companions="$tmp/tailcat"; fi
    install_binary_atomic "$bin" release "${resolved_version:-$VERSION}" "$companions" "$(binary_sha256 "$tmp/$asset")"
}

run_first_run_setup() {
    [ "$NO_SETUP" = "1" ] && {
        say "setup skipped (LABBY_INSTALL_NO_SETUP=1)"
        return 0
    }

    setup_binary="$INSTALL_DIR/labby"
    [ -x "$setup_binary" ] || fail "installed binary is not executable: $setup_binary"
    say ""
    say "[2/2] Configure Labby"

    if [ -n "$SETUP_ROLE" ]; then
        set -- setup --role "$SETUP_ROLE" --yes
        if [ "$SETUP_ROLE" = "server" ]; then
            set -- "$@" --deployment "${SETUP_DEPLOYMENT:-native}"
        elif [ -n "$SETUP_DEPLOYMENT" ]; then
            set -- "$@" --deployment "$SETUP_DEPLOYMENT"
        fi
        [ -z "$SETUP_HOST" ] || set -- "$@" --host "$SETUP_HOST"
        [ -z "$SETUP_PORT" ] || set -- "$@" --port "$SETUP_PORT"
        [ -z "$SETUP_SERVER_URL" ] || set -- "$@" --server-url "$SETUP_SERVER_URL"
        [ -z "$SETUP_PUBLIC_URL" ] || set -- "$@" --public-url "$SETUP_PUBLIC_URL"
        [ -z "$SETUP_AUTH" ] || set -- "$@" --auth "$SETUP_AUTH"
        [ -z "$SETUP_OAUTH" ] || set -- "$@" --oauth "$SETUP_OAUTH"
        case "$SETUP_DESKTOP" in
            1|true|yes) set -- "$@" --desktop ;;
            0|false|no|'') set -- "$@" --no-desktop ;;
            *) fail "LABBY_SETUP_DESKTOP must be 1/0, true/false, or yes/no" ;;
        esac
        [ "$SETUP_NO_BROWSER" != "1" ] || set -- "$@" --no-browser
        "$setup_binary" "$@"
        return
    fi

    if [ -t 0 ]; then
        "$setup_binary" setup
    elif tty </dev/tty >/dev/null 2>&1; then
        "$setup_binary" setup </dev/tty
    else
        fail "setup needs an interactive terminal or LABBY_SETUP_ROLE. Set LABBY_INSTALL_NO_SETUP=1 only when you intentionally want a binary-only install."
    fi
}

install_from_source() {
    command -v cargo >/dev/null 2>&1 || return 1
    say "no release asset available — building from source (this takes a while) ..."
    make_tmp_dir
    cargo_root="$CREATED_TMP_DIR"
    if [ "$VERSION" = "latest" ]; then
        command -v git >/dev/null 2>&1 || fail "git is required to resolve an immutable source revision"
        source_revision=$(git ls-remote "https://github.com/${REPO}" HEAD | awk 'NR == 1 { print $1 }')
        case "$source_revision" in
            '') fail "could not resolve the source repository HEAD" ;;
            *[!0-9a-f]*) fail "source repository returned an invalid revision" ;;
        esac
        [ "${#source_revision}" -eq 40 ] || [ "${#source_revision}" -eq 64 ] \
            || fail "source repository returned an invalid revision"
        cargo install --git "https://github.com/${REPO}" --rev "$source_revision" labby --bin labby --all-features --root "$cargo_root"
        source_identity="rev:${source_revision}"
    else
        cargo install --git "https://github.com/${REPO}" --tag "$VERSION" labby --bin labby --all-features --root "$cargo_root"
        source_identity="$VERSION"
    fi
    say "Source fallback installs the binary only; Tailcat companion assets require a matching prebuilt release."
    install_binary_atomic "$cargo_root/bin/labby" source "$source_identity"
}

main() {
    print_banner
    say "[1/2] Install verified Labby binary"
    acquire_transaction_lock
    if [ -d "$ACTIVATION_JOURNAL" ]; then
        recover_activation || fail "activation recovery FAILED; journal retained at $ACTIVATION_JOURNAL"
    fi
    if [ "$RECOVER_ONLY" = "1" ]; then
        return 0
    fi
    if [ "$ROLLBACK" = "1" ]; then
        rollback_offline
        say ""
        say "labby restored: $("$INSTALL_DIR/labby" --version 2>/dev/null || echo "$INSTALL_DIR/labby")"
        return 0
    fi
    if [ -n "$LOCAL_BINARY" ]; then
        install_local_binary
    elif [ -n "$LOCAL_SHA256" ]; then
        fail "LABBY_INSTALL_LOCAL_SHA256 requires LABBY_INSTALL_LOCAL_BINARY"
    elif install_from_release; then
        :
    elif [ "$ALLOW_SOURCE_FALLBACK" != "1" ]; then
        fail "could not install: release install failed and LABBY_ALLOW_SOURCE_FALLBACK=$ALLOW_SOURCE_FALLBACK disables source fallback.
Choose a supported prebuilt release or re-run with LABBY_ALLOW_SOURCE_FALLBACK=1 to build from source."
    elif install_from_source; then
        :
    else
        fail "could not install: no prebuilt release for $(uname -s)/$(uname -m) and no cargo toolchain found.
Install a Rust toolchain (https://rustup.rs) and re-run, or build from a clone:
  git clone https://github.com/${REPO} && cd labby && cargo install --path crates/labby --bin labby --all-features"
    fi

    if ! command -v labby >/dev/null 2>&1; then
        say ""
        say "NOTE: $INSTALL_DIR is not on your PATH. Add it, e.g.:"
        say "  export PATH=\"$INSTALL_DIR:\$PATH\""
    fi

    say ""
    say "labby installed: $("$INSTALL_DIR/labby" --version 2>/dev/null || echo "$INSTALL_DIR/labby")"

    # The setup process may elevate and may itself install/restart services. Do
    # not hold the binary activation lock across that independent transaction.
    release_transaction_lock
    run_first_run_setup
}

main "$@"
