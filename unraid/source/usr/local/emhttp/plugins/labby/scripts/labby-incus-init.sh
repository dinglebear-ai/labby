#!/bin/bash
# labby-incus-init.sh - idempotently converge the Incus-mode Labby gateway.
# This script is intentionally fail-closed: Unraid runs plugin event hooks as
# root, so ambiguous Incus state should stop here rather than modifying host
# networking or reusing unverified image bytes.
set -euo pipefail

EMHTTP="${EMHTTP:-/usr/local/emhttp/plugins/labby}"

# Source the private incus-unraid environment before labby.cfg so every Incus
# command below resolves to /usr/local/incus and the matching INCUS_DIR.
# shellcheck disable=SC1091
. "${EMHTTP}/scripts/labby-incus-env.sh"

CFG="${CFG:-/boot/config/plugins/labby/labby.cfg}"
LOG_TAG="labby-incus"
INCUS="${INCUS:-/usr/local/incus/bin/incus}"
READY_URL="http://127.0.0.1:8765/ready"
PROVISION_SCHEMA_VERSION="1"
PROVISION_SENTINEL="/var/lib/labby/provisioning-sentinel"
TS_AUTHKEY_PATH="/run/labby-ts-authkey"
IMAGE_PROP_VERSION="labby.image_version"
IMAGE_PROP_SHA256="labby.image_sha256"
PRIVATE_EGRESS_CIDRS="10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 100.64.0.0/10"

log() {
    logger -t "$LOG_TAG" "$*" 2>/dev/null || true
    printf 'labby-incus-init: %s\n' "$*"
}

fail() {
    log "FATAL: $*"
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

run_timeout() {
    timeout "$@"
}

incus_exec() {
    local seconds="$1"
    shift
    run_timeout "$seconds" "$INCUS" exec "$INCUS_CONTAINER_NAME" -- "$@"
}

cleanup_ts_authkey() {
    incus_exec 10 rm -f "$TS_AUTHKEY_PATH" >/dev/null 2>&1 || true
}

require_command awk
require_command curl
require_command flock
require_command grep
require_command ip
require_command sed
require_command sha256sum
require_command timeout

[ -f "$CFG" ] || fail "$CFG not found"
# shellcheck disable=SC1090
. "$CFG"

RUNTIME_MODE="${RUNTIME_MODE:-native}"
if [ "$RUNTIME_MODE" != "incus" ]; then
    log "RUNTIME_MODE=${RUNTIME_MODE} - nothing to do"
    exit 0
fi

INCUS_CONTAINER_NAME="${INCUS_CONTAINER_NAME:-labby-gateway}"
INCUS_IMAGE_VERSION="${INCUS_IMAGE_VERSION:-1.2.0}"
INCUS_IMAGE_SHA256="${INCUS_IMAGE_SHA256:-}"
INCUS_TS_AUTHKEY="${INCUS_TS_AUTHKEY:-}"
INCUS_BRIDGE_NAME="${INCUS_BRIDGE_NAME:-labbybr0}"
INCUS_BRIDGE_SUBNET="${INCUS_BRIDGE_SUBNET:-10.99.99.1/24}"
INCUS_EGRESS_POLICY="${INCUS_EGRESS_POLICY:-block-lan}"
LABBY_DIR="${LABBY_DIR:-/mnt/user/appdata/labby}"

STORAGE_POOL_NAME="labby-dir"
PROFILE_NAME="labby-gateway"
IMAGE_ALIAS="labby-gateway-${INCUS_IMAGE_VERSION}"
IMAGE_ASSET="labby-incus-x86_64-unknown-linux-gnu.tar.xz"
IMAGE_URL="https://github.com/jmagar/labby/releases/download/v${INCUS_IMAGE_VERSION}/${IMAGE_ASSET}"
IMAGE_CACHE_DIR="${LABBY_DIR}/incus-images"
IMAGE_CACHE_FILE="${IMAGE_CACHE_DIR}/labby-incus-${INCUS_IMAGE_VERSION}-x86_64-unknown-linux-gnu.tar.xz"
IMAGE_SHA_CACHE_FILE="${IMAGE_CACHE_DIR}/labby-incus-${INCUS_IMAGE_VERSION}-x86_64-unknown-linux-gnu.tar.xz.sha256"

validate_dns_label() {
    [[ "$1" =~ ^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$ ]]
}

validate_safe_token() {
    case "$1" in
        "" | *[!A-Za-z0-9._-]*) return 1 ;;
        *) return 0 ;;
    esac
}

validate_sha256() {
    [[ "$1" =~ ^[a-f0-9]{64}$ ]]
}

validate_version() {
    [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
}

validate_ipv4_cidr() {
    local cidr="$1"
    local ip prefix octet

    case "$cidr" in
        */*) ;;
        *) return 1 ;;
    esac

    ip="${cidr%/*}"
    prefix="${cidr#*/}"
    [[ "$prefix" =~ ^[0-9]+$ ]] || return 1
    [ "$prefix" -ge 1 ] && [ "$prefix" -le 30 ] || return 1

    IFS=. read -r o1 o2 o3 o4 <<EOF
$ip
EOF
    for octet in "$o1" "$o2" "$o3" "$o4"; do
        [[ "$octet" =~ ^[0-9]+$ ]] || return 1
        [ "$octet" -ge 0 ] && [ "$octet" -le 255 ] || return 1
    done
}

ipv4_to_int() {
    local ip="$1"
    local o1 o2 o3 o4

    IFS=. read -r o1 o2 o3 o4 <<EOF
$ip
EOF
    printf '%u\n' "$(( (o1 << 24) + (o2 << 16) + (o3 << 8) + o4 ))"
}

cidr_range() {
    local cidr="$1"
    local ip="${cidr%/*}"
    local prefix="${cidr#*/}"
    local ip_int mask start size end

    case "$cidr" in
        */*) ;;
        *) prefix=32 ;;
    esac

    ip_int="$(ipv4_to_int "$ip")"
    if [ "$prefix" -eq 0 ]; then
        mask=0
    else
        mask=$(( (0xffffffff << (32 - prefix)) & 0xffffffff ))
    fi
    start=$(( ip_int & mask ))
    size=$(( 1 << (32 - prefix) ))
    end=$(( start + size - 1 ))
    printf '%s %s\n' "$start" "$end"
}

cidrs_overlap() {
    local left="$1"
    local right="$2"
    local left_start left_end right_start right_end

    read -r left_start left_end <<EOF
$(cidr_range "$left")
EOF
    read -r right_start right_end <<EOF
$(cidr_range "$right")
EOF
    [ "$left_start" -le "$right_end" ] && [ "$right_start" -le "$left_end" ]
}

validate_inputs() {
    validate_dns_label "$INCUS_CONTAINER_NAME" \
        || fail "INCUS_CONTAINER_NAME must be a DNS label: ${INCUS_CONTAINER_NAME}"
    validate_version "$INCUS_IMAGE_VERSION" \
        || fail "INCUS_IMAGE_VERSION must be a plain X.Y.Z version number, got: ${INCUS_IMAGE_VERSION}"
    validate_sha256 "$INCUS_IMAGE_SHA256" \
        || fail "INCUS_IMAGE_SHA256 must be the pinned 64-character sha256 for v${INCUS_IMAGE_VERSION}"
    validate_safe_token "$INCUS_BRIDGE_NAME" \
        || fail "INCUS_BRIDGE_NAME contains unsupported characters: ${INCUS_BRIDGE_NAME}"
    validate_ipv4_cidr "$INCUS_BRIDGE_SUBNET" \
        || fail "INCUS_BRIDGE_SUBNET must be an IPv4 CIDR, got: ${INCUS_BRIDGE_SUBNET}"

    case "$INCUS_EGRESS_POLICY" in
        block-lan | allow-lan) ;;
        *) fail "INCUS_EGRESS_POLICY must be block-lan or allow-lan, got: ${INCUS_EGRESS_POLICY}" ;;
    esac

    case "$IMAGE_CACHE_DIR" in
        /boot/* | /boot)
            fail "Incus image cache would land on flash (${IMAGE_CACHE_DIR}); set LABBY_DIR to array-backed appdata"
            ;;
    esac
}

acquire_lock() {
    local lockfile="/var/run/labby-incus-init.lock"

    exec 200>"$lockfile"
    if ! flock -w 120 200; then
        fail "another labby-incus-init instance did not finish within 120s"
    fi
}

wait_for_incus() {
    local deadline=$((SECONDS + 30))

    while [ "$SECONDS" -lt "$deadline" ]; do
        if run_timeout 3 "$INCUS" info >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done

    fail "incusd did not become reachable after 30s; verify incus-unraid is installed, enabled, and the array is up"
}

ensure_storage_pool() {
    local dir_source

    if run_timeout 15 "$INCUS" storage show "$STORAGE_POOL_NAME" >/dev/null 2>&1; then
        return 0
    fi

    dir_source="$(dirname "$INCUS_DIR")/incus-storage-${STORAGE_POOL_NAME}"
    mkdir -p "$dir_source"
    log "creating storage pool ${STORAGE_POOL_NAME} at ${dir_source}"
    run_timeout 60 "$INCUS" storage create "$STORAGE_POOL_NAME" dir source="$dir_source" \
        || fail "failed to create Incus storage pool ${STORAGE_POOL_NAME}"
}

validate_bridge_subnet_collision() {
    local route
    local dest

    while IFS= read -r route; do
        dest="$(printf '%s\n' "$route" |
            awk '
                $1 == "default" { next }
                $1 ~ /^[0-9.]+(\/[0-9]+)?$/ { print $1; next }
                $2 ~ /^[0-9.]+(\/[0-9]+)?$/ { print $2; next }
            ')"
        [ -n "$dest" ] || continue
        case "$dest" in
            */*) ;;
            *) dest="${dest}/32" ;;
        esac
        if cidrs_overlap "$INCUS_BRIDGE_SUBNET" "$dest" &&
            ! printf '%s\n' "$route" | grep -q "dev ${INCUS_BRIDGE_NAME}\\b"; then
            fail "INCUS_BRIDGE_SUBNET ${INCUS_BRIDGE_SUBNET} collides with existing route: ${route}"
        fi
    done <<EOF
$(ip -4 route show 2>/dev/null || true)
EOF
}

network_managed() {
    run_timeout 15 "$INCUS" network show "$INCUS_BRIDGE_NAME" 2>/dev/null |
        awk '$1 == "managed:" { print $2; exit }'
}

ensure_bridge() {
    local managed
    local current_addr

    validate_bridge_subnet_collision

    managed="$(network_managed || true)"
    if [ "$managed" = "true" ]; then
        current_addr="$(run_timeout 15 "$INCUS" network get "$INCUS_BRIDGE_NAME" ipv4.address 2>/dev/null || true)"
        [ "$current_addr" = "$INCUS_BRIDGE_SUBNET" ] \
            || fail "${INCUS_BRIDGE_NAME} exists with ipv4.address=${current_addr}, expected ${INCUS_BRIDGE_SUBNET}"
        return 0
    fi

    if ip link show "$INCUS_BRIDGE_NAME" >/dev/null 2>&1 || [ "$managed" = "false" ]; then
        fail "${INCUS_BRIDGE_NAME} exists but is not Incus-managed; refusing to delete or modify an unmanaged host interface"
    fi

    log "creating Incus bridge ${INCUS_BRIDGE_NAME} (${INCUS_BRIDGE_SUBNET})"
    run_timeout 60 "$INCUS" network create "$INCUS_BRIDGE_NAME" --type=bridge \
        ipv4.address="$INCUS_BRIDGE_SUBNET" \
        ipv4.nat=true \
        ipv6.address=none \
        ipv6.nat=false \
        || fail "failed to create Incus bridge ${INCUS_BRIDGE_NAME}"
}

ensure_iptables_rule() {
    local cidr="$1"

    if iptables -C FORWARD -i "$INCUS_BRIDGE_NAME" -d "$cidr" -j REJECT >/dev/null 2>&1; then
        return 0
    fi
    iptables -I FORWARD 1 -i "$INCUS_BRIDGE_NAME" -d "$cidr" -j REJECT \
        || fail "failed to install LAN-egress reject rule for ${cidr}"
    iptables -C FORWARD -i "$INCUS_BRIDGE_NAME" -d "$cidr" -j REJECT >/dev/null 2>&1 \
        || fail "failed to verify LAN-egress reject rule for ${cidr}"
}

remove_iptables_rule() {
    local cidr="$1"

    while iptables -C FORWARD -i "$INCUS_BRIDGE_NAME" -d "$cidr" -j REJECT >/dev/null 2>&1; do
        iptables -D FORWARD -i "$INCUS_BRIDGE_NAME" -d "$cidr" -j REJECT \
            || fail "failed to remove LAN-egress reject rule for ${cidr}"
    done
}

apply_egress_policy() {
    local cidr

    case "$INCUS_EGRESS_POLICY" in
        allow-lan)
            require_command iptables
            for cidr in $PRIVATE_EGRESS_CIDRS; do
                remove_iptables_rule "$cidr"
            done
            log "egress policy allow-lan: ${INCUS_BRIDGE_NAME} NAT permits bridge-forwarded private LAN access by explicit operator config"
            ;;
        block-lan)
            require_command iptables
            for cidr in $PRIVATE_EGRESS_CIDRS; do
                ensure_iptables_rule "$cidr"
            done
            log "egress policy block-lan: installed and verified FORWARD rejects for bridge-forwarded private destination ranges; use Tailscale ACLs for tailnet policy"
            ;;
    esac
}

ensure_profile() {
    local profile_src="${EMHTTP}/incus/labby-gateway-profile.yaml"
    local current_network

    [ -f "$profile_src" ] || fail "${profile_src} not found"

    if ! run_timeout 15 "$INCUS" profile show "$PROFILE_NAME" >/dev/null 2>&1; then
        log "creating Incus profile ${PROFILE_NAME}"
        run_timeout 30 "$INCUS" profile create "$PROFILE_NAME" \
            || fail "failed to create Incus profile ${PROFILE_NAME}"
    fi

    sed "s/^    pool: .*/    pool: ${STORAGE_POOL_NAME}/" "$profile_src" |
        run_timeout 60 "$INCUS" profile edit "$PROFILE_NAME" \
        || fail "failed to update Incus profile ${PROFILE_NAME}"

    current_network="$(run_timeout 15 "$INCUS" profile device get "$PROFILE_NAME" eth0 network 2>/dev/null || true)"
    if [ "$current_network" != "$INCUS_BRIDGE_NAME" ]; then
        run_timeout 20 "$INCUS" profile device remove "$PROFILE_NAME" eth0 >/dev/null 2>&1 || true
        run_timeout 30 "$INCUS" profile device add "$PROFILE_NAME" eth0 nic network="$INCUS_BRIDGE_NAME" \
            || fail "failed to attach ${INCUS_BRIDGE_NAME} to profile ${PROFILE_NAME}"
    fi
}

write_checksum_cache() {
    printf '%s  %s\n' "$INCUS_IMAGE_SHA256" "$(basename "$IMAGE_CACHE_FILE")" > "$IMAGE_SHA_CACHE_FILE"
}

verify_image_file() {
    local file="$1"
    local actual

    [ -f "$file" ] || return 1
    actual="$(sha256sum "$file" | awk '{ print $1 }')"
    [ "$actual" = "$INCUS_IMAGE_SHA256" ]
}

download_image_once() {
    local tmp="${IMAGE_CACHE_FILE}.tmp.$$"

    rm -f "$tmp"
    log "downloading ${IMAGE_ASSET} for v${INCUS_IMAGE_VERSION}"
    if ! curl -fsSL --connect-timeout 15 --max-time 600 --retry 2 --retry-delay 2 -o "$tmp" "$IMAGE_URL"; then
        rm -f "$tmp"
        return 1
    fi
    if ! verify_image_file "$tmp"; then
        rm -f "$tmp"
        return 1
    fi
    mv "$tmp" "$IMAGE_CACHE_FILE"
    write_checksum_cache
}

ensure_image_cache() {
    local attempt=1

    mkdir -p "$IMAGE_CACHE_DIR"
    if [ -f "$IMAGE_CACHE_FILE" ]; then
        if verify_image_file "$IMAGE_CACHE_FILE"; then
            write_checksum_cache
            return 0
        fi
        log "cached image failed sha256 verification; removing ${IMAGE_CACHE_FILE}"
        rm -f "$IMAGE_CACHE_FILE" "$IMAGE_SHA_CACHE_FILE"
    fi

    while [ "$attempt" -le 2 ]; do
        if download_image_once; then
            return 0
        fi
        log "download attempt ${attempt} failed sha256/download verification"
        attempt=$((attempt + 1))
    done

    fail "could not download and verify ${IMAGE_URL} against pinned sha256 ${INCUS_IMAGE_SHA256}"
}

ensure_image_imported() {
    local actual_version
    local actual_sha256

    if run_timeout 15 "$INCUS" image info -- "local:${IMAGE_ALIAS}" >/dev/null 2>&1; then
        actual_version="$(run_timeout 15 "$INCUS" image get-property "local:${IMAGE_ALIAS}" "$IMAGE_PROP_VERSION" 2>/dev/null || true)"
        actual_sha256="$(run_timeout 15 "$INCUS" image get-property "local:${IMAGE_ALIAS}" "$IMAGE_PROP_SHA256" 2>/dev/null || true)"
        [ "$actual_version" = "$INCUS_IMAGE_VERSION" ] && [ "$actual_sha256" = "$INCUS_IMAGE_SHA256" ] \
            || fail "existing Incus image alias ${IMAGE_ALIAS} is not stamped with the configured version/SHA256; delete it with 'incus image delete ${IMAGE_ALIAS}' and rerun so verified bytes are imported"
        return 0
    fi

    ensure_image_cache
    log "importing verified image cache as ${IMAGE_ALIAS}"
    run_timeout 300 "$INCUS" image import "$IMAGE_CACHE_FILE" --alias "$IMAGE_ALIAS" \
        || fail "failed to import Incus image ${IMAGE_CACHE_FILE}"
    run_timeout 30 "$INCUS" image set-property "local:${IMAGE_ALIAS}" \
        "${IMAGE_PROP_VERSION}=${INCUS_IMAGE_VERSION}" \
        "${IMAGE_PROP_SHA256}=${INCUS_IMAGE_SHA256}" \
        || fail "failed to stamp Incus image ${IMAGE_ALIAS} with version/SHA256 metadata"
    ensure_image_imported
}

instance_exists() {
    local info

    info="$(run_timeout 15 "$INCUS" info -- "$INCUS_CONTAINER_NAME" 2>&1)" && return 0
    case "$info" in
        *"Instance not found"*) return 1 ;;
        *) fail "failed to query Incus instance ${INCUS_CONTAINER_NAME}: ${info}" ;;
    esac
}

instance_state() {
    local info
    local state

    info="$(run_timeout 15 "$INCUS" info -- "$INCUS_CONTAINER_NAME" 2>&1)" || {
        case "$info" in
            *"Instance not found"*) return 1 ;;
            *) fail "failed to query Incus instance ${INCUS_CONTAINER_NAME}: ${info}" ;;
        esac
    }
    state="$(printf '%s\n' "$info" | awk -F': ' '$1 == "Status" { print toupper($2); exit }')"
    [ -n "$state" ] || fail "Incus state response for ${INCUS_CONTAINER_NAME} did not include a Status line: ${info}"
    printf '%s\n' "$state"
}

ensure_container_running() {
    local state
    local actual_version
    local actual_sha256

    if ! instance_exists; then
        ensure_image_imported
        log "launching ${INCUS_CONTAINER_NAME} from ${IMAGE_ALIAS}"
        run_timeout 300 "$INCUS" launch --profile "$PROFILE_NAME" -- "local:${IMAGE_ALIAS}" "$INCUS_CONTAINER_NAME" \
            || fail "failed to launch ${INCUS_CONTAINER_NAME}"
        run_timeout 30 "$INCUS" config set -- "$INCUS_CONTAINER_NAME" "user.labby.image_version=${INCUS_IMAGE_VERSION}" \
            || fail "failed to annotate ${INCUS_CONTAINER_NAME} with image version"
        run_timeout 30 "$INCUS" config set -- "$INCUS_CONTAINER_NAME" "user.labby.image_sha256=${INCUS_IMAGE_SHA256}" \
            || fail "failed to annotate ${INCUS_CONTAINER_NAME} with image sha256"
        return 0
    fi

    actual_version="$(run_timeout 15 "$INCUS" config get -- "$INCUS_CONTAINER_NAME" user.labby.image_version 2>/dev/null || true)"
    actual_sha256="$(run_timeout 15 "$INCUS" config get -- "$INCUS_CONTAINER_NAME" user.labby.image_sha256 2>/dev/null || true)"
    [ "$actual_version" = "$INCUS_IMAGE_VERSION" ] && [ "$actual_sha256" = "$INCUS_IMAGE_SHA256" ] \
        || fail "existing Incus container ${INCUS_CONTAINER_NAME} was created for image version/SHA ${actual_version:-unknown}/${actual_sha256:-unknown}, not ${INCUS_IMAGE_VERSION}/${INCUS_IMAGE_SHA256}; delete/recreate it after backing up any needed state"

    state="$(instance_state || true)"
    if [ "$state" != "RUNNING" ]; then
        log "starting existing container ${INCUS_CONTAINER_NAME}"
        run_timeout 120 "$INCUS" start -- "$INCUS_CONTAINER_NAME" \
            || fail "failed to start ${INCUS_CONTAINER_NAME}"
    fi
}

wait_for_network() {
    local deadline=$((SECONDS + 60))

    while [ "$SECONDS" -lt "$deadline" ]; do
        if incus_exec 3 sh -c "ip -4 addr show dev eth0 | grep -q 'inet '" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done

    fail "${INCUS_CONTAINER_NAME} did not acquire an IPv4 address on eth0 after 60s"
}

container_ready() {
    incus_exec 10 curl -fsS -m 3 "$READY_URL" >/dev/null 2>&1
}

wait_for_ready() {
    local deadline=$((SECONDS + 60))

    while [ "$SECONDS" -lt "$deadline" ]; do
        if container_ready; then
            return 0
        fi
        sleep 1
    done

    fail "${INCUS_CONTAINER_NAME} did not become ready at ${READY_URL} after 60s"
}

ensure_service_active() {
    incus_exec 20 systemctl is-active --quiet labby.service \
        || fail "labby.service is not active inside ${INCUS_CONTAINER_NAME}"
}

tailscale_has_ip() {
    incus_exec 20 tailscale ip -4 >/dev/null 2>&1
}

redact_ts_authkey_to() {
    local dest="$1"
    local mode=""
    local owner=""
    local group=""

    mode="$(stat -c '%a' "$CFG" 2>/dev/null || true)"
    owner="$(stat -c '%u' "$CFG" 2>/dev/null || true)"
    group="$(stat -c '%g' "$CFG" 2>/dev/null || true)"
    awk '
        /^INCUS_TS_AUTHKEY=/ {
            comment = ""
            if (match($0, /[[:space:]]+#.*/)) {
                comment = substr($0, RSTART)
            }
            if (comment == "") {
                comment = "                   # incus mode only - write-only one-shot Tailscale auth key; clear/redact after attempted use"
            }
            print "INCUS_TS_AUTHKEY=\"\"" comment
            cleared = 1
            next
        }
        { print }
        END {
            if (!cleared) {
                print "INCUS_TS_AUTHKEY=\"\"                   # incus mode only - write-only one-shot Tailscale auth key; clear/redact after attempted use"
            }
        }
    ' "$CFG" > "$dest"
    [ -n "$mode" ] && chmod "$mode" "$dest" 2>/dev/null || true
    if [ -n "$owner" ] && [ -n "$group" ]; then
        chown "$owner:$group" "$dest" 2>/dev/null || true
    fi
}

clear_stored_ts_authkey() {
    local backup="${CFG}.bak"
    local backup_tmp="${CFG}.bak.tmp.$$"
    local tmp="${CFG}.tmp.$$"

    [ -f "$CFG" ] || fail "${CFG} disappeared before INCUS_TS_AUTHKEY could be cleared"
    rm -f "$backup_tmp" "$tmp"

    # Backup-first, but redact the one-shot secret in the backup too so an
    # attempted use does not leave the auth key behind in labby.cfg.bak.
    if ! redact_ts_authkey_to "$backup_tmp"; then
        rm -f "$backup_tmp" "$tmp"
        fail "failed to prepare redacted backup for ${CFG}"
    fi
    if ! cp -p "$backup_tmp" "$tmp"; then
        rm -f "$backup_tmp" "$tmp"
        fail "failed to prepare redacted ${CFG}"
    fi
    if ! mv -f "$backup_tmp" "$backup"; then
        rm -f "$backup_tmp" "$tmp"
        fail "failed to write redacted backup ${backup}"
    fi

    if ! mv -f "$tmp" "$CFG"; then
        rm -f "$tmp"
        fail "failed to atomically clear INCUS_TS_AUTHKEY from ${CFG}"
    fi

    log "cleared INCUS_TS_AUTHKEY from ${CFG} after attempted Tailscale use"
}

consume_tailscale_authkey() {
    local status

    [ -n "$INCUS_TS_AUTHKEY" ] || return 0

    incus_exec 20 sh -c "command -v tailscale >/dev/null 2>&1" \
        || fail "tailscale is missing from the baked image; runtime curl-pipe installation is forbidden"

    if tailscale_has_ip; then
        log "tailscale already has an IPv4 address; not reusing supplied one-shot INCUS_TS_AUTHKEY"
        clear_stored_ts_authkey
        return 0
    fi

    cleanup_ts_authkey
    if ! printf '%s' "$INCUS_TS_AUTHKEY" |
        run_timeout 30 "$INCUS" exec "$INCUS_CONTAINER_NAME" -- sh -c "umask 077; cat > ${TS_AUTHKEY_PATH} && chmod 0600 ${TS_AUTHKEY_PATH}"; then
        cleanup_ts_authkey
        fail "failed to write mode-0600 Tailscale auth key inside ${INCUS_CONTAINER_NAME}"
    fi

    if ! incus_exec 10 sh -c "[ \"\$(stat -c %a ${TS_AUTHKEY_PATH})\" = 600 ]"; then
        cleanup_ts_authkey
        fail "Tailscale auth key temp file was not mode 0600"
    fi

    set +e
    incus_exec 180 tailscale up "--auth-key=file:${TS_AUTHKEY_PATH}" "--hostname=${INCUS_CONTAINER_NAME}"
    status=$?
    set -e
    cleanup_ts_authkey
    if [ "$status" -ne 0 ]; then
        clear_stored_ts_authkey
        fail "tailscale up failed for ${INCUS_CONTAINER_NAME}; cleared stored one-shot INCUS_TS_AUTHKEY"
    fi

    tailscale_has_ip || fail "tailscale did not report an IPv4 address after join"
    log "tailscale joined and ${TS_AUTHKEY_PATH} removed"
    clear_stored_ts_authkey
}

container_labby_version() {
    incus_exec 20 labby --version |
        awk '{ print $2; exit }'
}

desired_sentinel() {
    local labby_version="$1"

    printf 'image_version=%s\n' "$INCUS_IMAGE_VERSION"
    printf 'image_sha256=%s\n' "$INCUS_IMAGE_SHA256"
    printf 'labby_version=%s\n' "$labby_version"
    printf 'provision_schema=%s\n' "$PROVISION_SCHEMA_VERSION"
}

current_sentinel() {
    incus_exec 15 sh -c "cat ${PROVISION_SENTINEL} 2>/dev/null || true"
}

sentinel_matches() {
    local labby_version="$1"
    local desired
    local current

    desired="$(desired_sentinel "$labby_version")"
    current="$(current_sentinel)"
    [ "$current" = "$desired" ]
}

write_sentinel() {
    local labby_version="$1"

    desired_sentinel "$labby_version" |
        run_timeout 30 "$INCUS" exec "$INCUS_CONTAINER_NAME" -- sh -c "install -d -m 0755 \"\$(dirname ${PROVISION_SENTINEL})\" && cat > ${PROVISION_SENTINEL}" \
        || fail "failed to write provisioning sentinel"
}

converge_provisioning() {
    local labby_version

    labby_version="$(container_labby_version)"
    [ -n "$labby_version" ] || fail "could not determine baked labby binary version inside ${INCUS_CONTAINER_NAME}"

    if container_ready && sentinel_matches "$labby_version"; then
        ensure_service_active
        log "${INCUS_CONTAINER_NAME} is already running, ready, and provision sentinel matches; skipping labby setup --provision --yes"
        return 0
    fi

    if [ "${LABBY_ARRAY_START:-0}" = "1" ] && container_ready; then
        ensure_service_active
        fail "${INCUS_CONTAINER_NAME} is ready but provision sentinel drifted; use Settings > Labby Start/Restart or SSH rc.labby restart to reprovision outside the array-start hook"
    fi

    if sentinel_matches "$labby_version"; then
        log "provisioning sentinel matches image=${INCUS_IMAGE_VERSION}, sha256=${INCUS_IMAGE_SHA256}, labby=${labby_version}, schema=${PROVISION_SCHEMA_VERSION}; restarting service without reprovisioning"
        incus_exec 120 systemctl restart labby.service \
            || fail "labby.service restart failed inside ${INCUS_CONTAINER_NAME}"
    else
        log "running labby setup --provision --yes inside ${INCUS_CONTAINER_NAME}"
        incus_exec 900 labby setup --provision --yes \
            || fail "labby setup --provision --yes failed inside ${INCUS_CONTAINER_NAME}"
        incus_exec 120 systemctl enable --now labby.service \
            || fail "failed to enable/start labby.service inside ${INCUS_CONTAINER_NAME}"
        write_sentinel "$labby_version"
    fi

    ensure_service_active
    wait_for_ready
}

main() {
    validate_inputs
    acquire_lock
    wait_for_incus
    if [ "${LABBY_ARRAY_START:-0}" = "1" ] && ! instance_exists; then
        log "array-start autostart skipped first-time Incus bootstrap for ${INCUS_CONTAINER_NAME}; use Settings > Labby Start or /etc/rc.d/rc.labby start once to create it"
        exit 0
    fi
    ensure_storage_pool
    ensure_bridge
    apply_egress_policy
    ensure_profile
    ensure_container_running
    wait_for_network
    consume_tailscale_authkey
    converge_provisioning
    log "labby gateway ready inside ${INCUS_CONTAINER_NAME}"
}

if [ "${LABBY_INCUS_INIT_LIBRARY:-0}" != "1" ]; then
    main "$@"
fi
