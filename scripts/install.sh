#!/bin/sh
# Install labby — the Lab homelab control plane binary.
#
# Download labby-install.sh plus its checksum from an explicit release, verify
# its GitHub attestation and digest, then run: sh ./labby-install.sh
#
# Downloads the latest GitHub release archive for this platform, verifies its
# SHA-256, and installs the binary to ~/.local/bin/labby. When explicitly
# enabled with LABBY_ALLOW_SOURCE_FALLBACK=1, a release failure falls back to
# `cargo install --git` if a Rust toolchain is available.
#
# This script's ONLY job is bootstrap: getting `labby` onto PATH. Everything
# after that is owned by the binary — run `labby setup` for the first-run flow.
#
# Environment overrides:
#   LABBY_INSTALL_DIR     install directory       (default: ~/.local/bin)
#   LABBY_INSTALL_REPO    owner/repo to fetch     (default: dinglebear-ai/labby)
#   LABBY_INSTALL_VERSION release tag, e.g. v0.22.2 (default: latest)
#   LABBY_ALLOW_SOURCE_FALLBACK allow cargo fallback after release failure (default: 0)
#   LABBY_INSTALL_ROLLBACK restore the previous verified binary offline (default: 0)
#   LABBY_INSTALL_LOCAL_BINARY install an exact local candidate (requires SHA-256)
#   LABBY_INSTALL_LOCAL_SHA256 expected digest for LABBY_INSTALL_LOCAL_BINARY

set -eu

REPO="${LABBY_INSTALL_REPO:-dinglebear-ai/labby}"
INSTALL_DIR="${LABBY_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${LABBY_INSTALL_VERSION:-latest}"
ALLOW_SOURCE_FALLBACK="${LABBY_ALLOW_SOURCE_FALLBACK:-0}"
ROLLBACK="${LABBY_INSTALL_ROLLBACK:-0}"
LOCAL_BINARY="${LABBY_INSTALL_LOCAL_BINARY:-}"
LOCAL_SHA256="${LABBY_INSTALL_LOCAL_SHA256:-}"
INSTALL_METADATA_DIR="$INSTALL_DIR/.labby-install"
ARTIFACTS_DIR="$INSTALL_METADATA_DIR/artifacts"
RECEIPT_PATH="$INSTALL_METADATA_DIR/receipt"
PREVIOUS_RECEIPT_PATH="$INSTALL_METADATA_DIR/previous-receipt"
ACTIVATION_JOURNAL="$INSTALL_METADATA_DIR/activation-journal"
TMP_DIRS=""
CREATED_TMP_DIR=""

cleanup() {
    for dir in $TMP_DIRS; do
        rm -rf "$dir"
    done
}
trap cleanup EXIT

make_tmp_dir() {
    CREATED_TMP_DIR="$(mktemp -d)"
    TMP_DIRS="${TMP_DIRS} ${CREATED_TMP_DIR}"
}

say() { printf '%s\n' "$*" >&2; }
fail() { say "install.sh: $*"; exit 1; }

target_triple() {
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os" in
        Linux)
            case "$arch" in
                x86_64) echo "x86_64-unknown-linux-gnu" ;;
                *) fail "unsupported platform ${os}/${arch}; supported: Linux/x86_64" ;;
            esac
            ;;
        Darwin)
            case "$arch" in
                arm64) echo "aarch64-apple-darwin" ;;
                *) fail "unsupported platform ${os}/${arch}; supported: macOS/arm64" ;;
            esac
            ;;
        *) fail "unsupported platform ${os}/${arch}; supported: Linux/x86_64 and macOS/arm64" ;;
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
    command -v gh >/dev/null 2>&1 \
        || fail "GitHub CLI (gh) is required to verify release provenance"
    gh attestation verify "$artifact" \
        --repo "$REPO" \
        --signer-workflow "$REPO/.github/workflows/release.yml" \
        --source-ref "refs/tags/$resolved" \
        --deny-self-hosted-runners >/dev/null \
        || fail "GitHub provenance verification FAILED for $asset"
    say "GitHub provenance verified"
}

latest_release_with_asset() {
    # GitHub's "latest" release may point at non-binary artifacts such as the
    # Incus image release. Pick the newest release that actually contains the
    # platform binary archive we are about to download.
    # $1 = asset name
    curl -fsSL --connect-timeout 10 --max-time 300 --retry 3 "https://api.github.com/repos/${REPO}/releases?per_page=20" |
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

binary_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        fail "no sha256sum/shasum found; artifact identity cannot be recorded"
    fi
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
    rm -rf "$ACTIVATION_JOURNAL" || {
        say "activation recovery FAILED removing completed journal"
        return 1
    }
    say "interrupted installation transaction restored"
}

write_activation_state() {
    state_tmp="$ACTIVATION_JOURNAL/.state.$$"
    printf '%s\n' "$1" >"$state_tmp"
    chmod 600 "$state_tmp"
    mv -f "$state_tmp" "$ACTIVATION_JOURNAL/state"
}

install_binary_atomic() {
    # $1 = source binary, $2 = provenance, $3 = resolved version.
    source_binary=$1
    install_source=$2
    resolved_version=$3
    mkdir -p "$INSTALL_DIR" "$ARTIFACTS_DIR"
    chmod 700 "$INSTALL_METADATA_DIR" "$ARTIFACTS_DIR"
    recover_activation || fail "activation recovery FAILED; journal retained at $ACTIVATION_JOURNAL"
    digest=$(binary_sha256 "$source_binary")
    artifact_dir="$ARTIFACTS_DIR/$digest"
    artifact="$artifact_dir/labby"
    if [ ! -f "$artifact" ]; then
        mkdir -p "$artifact_dir"
        chmod 700 "$artifact_dir"
        artifact_tmp=$(mktemp "$artifact_dir/.labby.XXXXXX")
        install -m 755 "$source_binary" "$artifact_tmp"
        mv -f "$artifact_tmp" "$artifact"
    elif [ "$(binary_sha256 "$artifact")" != "$digest" ]; then
        fail "cached artifact digest does not match its content: $digest"
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
    write_activation_state prepared

    if ! (
        mv -f "$activation_dir/new-binary" "$INSTALL_DIR/labby" &&
        write_activation_state binary-activated &&
        { [ ! -f "$activation_dir/new-previous" ] || mv -f "$activation_dir/new-previous" "$PREVIOUS_RECEIPT_PATH"; } &&
        write_activation_state previous-receipt-activated &&
        mv -f "$activation_dir/receipt" "$RECEIPT_PATH"
    ); then
        say "activation failed; restoring the complete prior installation transaction"
        recover_activation || fail "activation recovery FAILED; journal retained at $ACTIVATION_JOURNAL"
        return 1
    fi
    write_activation_state receipt-activated
    rm -rf "$ACTIVATION_JOURNAL"
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
    install_binary_atomic "$local_staged" local "$VERSION"
}

rollback_offline() {
    [ -f "$PREVIOUS_RECEIPT_PATH" ] || fail "no previous verified installation is available for offline rollback"
    prior_digest=$(receipt_field sha256 "$PREVIOUS_RECEIPT_PATH")
    prior_source=$(receipt_field source "$PREVIOUS_RECEIPT_PATH")
    prior_requested=$(receipt_field requested_version "$PREVIOUS_RECEIPT_PATH")
    prior_resolved=$(receipt_field resolved_version "$PREVIOUS_RECEIPT_PATH")
    [ "${#prior_digest}" -eq 64 ] || fail "previous install receipt has an invalid artifact digest"
    case "$prior_digest" in *[!0-9a-f]*) fail "previous install receipt has an invalid artifact digest" ;; esac
    prior_artifact="$ARTIFACTS_DIR/$prior_digest/labby"
    [ -f "$prior_artifact" ] || fail "previous verified artifact is unavailable: $prior_digest"
    actual_digest=$(binary_sha256 "$prior_artifact")
    [ "$actual_digest" = "$prior_digest" ] || fail "previous artifact digest does not match its receipt"

    VERSION=$prior_requested
    install_binary_atomic "$prior_artifact" "$prior_source" "$prior_resolved"
    say "restored verified installation ${prior_resolved} (${prior_digest}) without network access"
}

install_from_release() {
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

    say "downloading ${base}/${asset} ..."
    curl -fsSL --connect-timeout 10 --max-time 300 --retry 3 -o "$tmp/$asset" "${base}/${asset}" || return 1
    if curl -fsSL --connect-timeout 10 --max-time 300 --retry 3 -o "$tmp/$asset.sha256" "${base}/${asset}.sha256"; then
        sha256_check "$tmp/$asset" "$tmp/$asset.sha256" \
            || fail "checksum verification FAILED for $asset — aborting"
        say "sha256 verified"
    else
        fail "no .sha256 asset published for $asset; release installs require checksum verification"
    fi

    verify_release_provenance "$tmp/$asset" "${resolved_version:-$VERSION}"

    tar -xzf "$tmp/$asset" -C "$tmp"
    bin="$(find "$tmp" -type f -name labby | head -n 1)"
    [ -n "$bin" ] || fail "archive $asset did not contain a 'labby' binary"

    install_binary_atomic "$bin" release "${resolved_version:-$VERSION}"
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
    install_binary_atomic "$cargo_root/bin/labby" source "$source_identity"
}

main() {
    if [ -d "$ACTIVATION_JOURNAL" ]; then
        recover_activation || fail "activation recovery FAILED; journal retained at $ACTIVATION_JOURNAL"
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
    say "next: run 'labby setup' to start the first-run flow"
}

main "$@"
