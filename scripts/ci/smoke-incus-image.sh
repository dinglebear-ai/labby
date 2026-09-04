#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

export_dir="${EXPORT_DIR:-$repo_root/target/incus-image-dist}"
image_alias="${IMAGE_ALIAS:-labby-incus-smoke-$$}"
container_name="${SMOKE_CONTAINER_NAME:-labby-incus-image-smoke-$$}"
bootstrap_container_name="${SMOKE_BOOTSTRAP_CONTAINER_NAME:-${container_name}-bootstrap}"
profile_name="${SMOKE_PROFILE_NAME:-labby-gateway-smoke-$$}"
bootstrap_profile_name="${SMOKE_BOOTSTRAP_PROFILE_NAME:-${profile_name}-bootstrap}"
profile_yaml="${SMOKE_PROFILE_YAML:-$repo_root/config/incus/labby-gateway-profile.yaml}"
backup_yaml="${SMOKE_BACKUP_YAML:-$repo_root/config/incus/labby-backup.yaml}"
image_tar="${IMAGE_TAR:-}"
expect_android_sdk="${SMOKE_EXPECT_ANDROID_SDK:-${LABBY_ENABLE_ANDROID_SDK:-0}}"

log() {
    printf '[labby-incus] %s\n' "$*"
}

die() {
    printf '[labby-incus] error: %s\n' "$*" >&2
    exit 1
}

have() {
    command -v "$1" >/dev/null 2>&1
}

systemd_is_pid_one() {
    local init_comm=""
    [[ -r /proc/1/comm ]] || return 1
    IFS= read -r init_comm </proc/1/comm || return 1
    [[ "$init_comm" == "systemd" ]]
}

sudo_cmd() {
    if [[ "$(id -u)" -eq 0 ]]; then
        "$@"
    else
        sudo "$@"
    fi
}

INCUS_USE_SUDO=0
MANUAL_INCUS_PID=""
MANUAL_INCUS_DIR=""
MANUAL_INCUS_DIR_SUFFIX="/run/seccomp.socket"
MANUAL_INCUS_LOG=""
MANUAL_INCUS_STDIO=""
SMOKE_PROFILE_CREATED=0
SMOKE_RESOURCES_OWNED=0
SMOKE_IMAGE_FINGERPRINT=""
SMOKE_IMAGE_IMPORTED=0
SMOKE_RENDERED_PROFILE=""
SYSTEMD_INCUS_SERVICE_STARTED=0
SYSTEMD_INCUS_SOCKET_STARTED=0

incus_cmd() {
    if [[ "$INCUS_USE_SUDO" == "1" ]]; then
        if [[ -n "${INCUS_DIR:-}" ]]; then
            sudo env INCUS_DIR="$INCUS_DIR" incus "$@"
        else
            sudo incus "$@"
        fi
    else
        incus "$@"
    fi
}

cleanup() {
    local status="$?"
    trap - EXIT
    set +e

    if [[ "$SMOKE_RESOURCES_OWNED" == "1" ]] && incus_cmd info >/dev/null 2>&1; then
        incus_cmd delete "$bootstrap_container_name" --force >/dev/null 2>&1 || true
        incus_cmd delete "$container_name" --force >/dev/null 2>&1 || true
        incus_cmd profile delete "$bootstrap_profile_name" >/dev/null 2>&1 || true
        if [[ "$SMOKE_PROFILE_CREATED" == "1" ]]; then
            incus_cmd profile delete "$profile_name" >/dev/null 2>&1 || true
        fi
        incus_cmd image alias delete "$image_alias" >/dev/null 2>&1 || true
        if [[ "$SMOKE_IMAGE_IMPORTED" == "1" && -n "$SMOKE_IMAGE_FINGERPRINT" ]]; then
            incus_cmd image delete "$SMOKE_IMAGE_FINGERPRINT" >/dev/null 2>&1 || true
        fi
    fi

    if [[ "$SYSTEMD_INCUS_SERVICE_STARTED" == "1" ]]; then
        sudo_cmd systemctl stop incus.service >/dev/null 2>&1 || true
    fi
    if [[ "$SYSTEMD_INCUS_SOCKET_STARTED" == "1" ]]; then
        sudo_cmd systemctl stop incus.socket >/dev/null 2>&1 || true
    fi

    if [[ -n "$MANUAL_INCUS_PID" ]] && sudo_cmd kill -0 "$MANUAL_INCUS_PID" >/dev/null 2>&1; then
        sudo_cmd kill -TERM "$MANUAL_INCUS_PID" >/dev/null 2>&1 || true
        for _ in $(seq 1 20); do
            sudo_cmd kill -0 "$MANUAL_INCUS_PID" >/dev/null 2>&1 || break
            sleep 1
        done
        if sudo_cmd kill -0 "$MANUAL_INCUS_PID" >/dev/null 2>&1; then
            sudo_cmd kill -KILL "$MANUAL_INCUS_PID" >/dev/null 2>&1 || true
        fi
        wait "$MANUAL_INCUS_PID" >/dev/null 2>&1 || true
    fi

    if [[ "$status" -ne 0 ]]; then
        if [[ -f "$MANUAL_INCUS_STDIO" ]]; then
            tail -n 80 "$MANUAL_INCUS_STDIO" >&2 || true
        fi
        if [[ -n "$MANUAL_INCUS_LOG" ]]; then
            sudo_cmd tail -n 80 "$MANUAL_INCUS_LOG" >&2 || true
        fi
    fi
    if [[ -n "$SMOKE_RENDERED_PROFILE" ]]; then
        rm -f "$SMOKE_RENDERED_PROFILE" || true
    fi
    if [[ -n "$MANUAL_INCUS_STDIO" ]]; then
        rm -f "$MANUAL_INCUS_STDIO" || true
    fi
    if [[ -n "$MANUAL_INCUS_LOG" ]]; then
        sudo_cmd rm -f "$MANUAL_INCUS_LOG" || true
    fi
    if [[ -n "$MANUAL_INCUS_DIR" ]]; then
        sudo_cmd rm -rf "$MANUAL_INCUS_DIR" || true
    fi

    exit "$status"
}
trap cleanup EXIT

case "$expect_android_sdk" in
    0|1) ;;
    *) die "SMOKE_EXPECT_ANDROID_SDK must be 0 or 1" ;;
esac

install_incus_if_needed() {
    if have incus; then
        return
    fi

    have apt-get || die "incus is not installed and this script only knows apt-get installation"
    log "installing incus"
    sudo_cmd apt-get update
    sudo_cmd apt-get install -y incus uidmap squashfs-tools
}

incus_daemon_binary() {
    local candidate
    for candidate in /opt/incus/lib/systemd/incusd /opt/incus/bin/incusd; do
        if [[ -x "$candidate" ]]; then
            printf '%s\n' "$candidate"
            return
        fi
    done
    candidate="$(command -v incusd 2>/dev/null || true)"
    [[ -n "$candidate" ]] || return 1
    printf '%s\n' "$candidate"
}

sudo_incus_info() {
    have sudo || return 1
    if [[ -n "${INCUS_DIR:-}" ]]; then
        sudo env INCUS_DIR="$INCUS_DIR" incus info
    else
        sudo incus info
    fi
}

incus_has_storage_pool() {
    local pool
    pool="$(incus_cmd storage list --format csv 2>/dev/null | awk -F, 'NF {print $1; exit}' || true)"
    [[ -n "$pool" ]]
}

wait_for_incus_client() {
    for _ in $(seq 1 60); do
        if incus info >/dev/null 2>&1; then
            INCUS_USE_SUDO=0
            return 0
        fi
        if sudo_incus_info >/dev/null 2>&1; then
            INCUS_USE_SUDO=1
            return 0
        fi
        if [[ -n "$MANUAL_INCUS_PID" ]] \
            && ! sudo_cmd kill -0 "$MANUAL_INCUS_PID" >/dev/null 2>&1; then
            return 1
        fi
        sleep 1
    done
    return 1
}

start_manual_incus() {
    local daemon manual_base manual_parent
    daemon="$(incus_daemon_binary)" || die "could not locate the Incus daemon binary"
    MANUAL_INCUS_DIR="${INCUS_SMOKE_DIR:-/var/tmp/labby-incus-$$}"
    [[ "$MANUAL_INCUS_DIR" == /* ]] \
        || die "INCUS_SMOKE_DIR must be an absolute path: $MANUAL_INCUS_DIR"
    manual_base="${MANUAL_INCUS_DIR##*/}"
    [[ "$manual_base" == labby-incus-* ]] \
        || die "INCUS_SMOKE_DIR basename must start with labby-incus-: $MANUAL_INCUS_DIR"
    manual_parent="${MANUAL_INCUS_DIR%/*}"
    [[ -d "$manual_parent" ]] \
        || die "INCUS_SMOKE_DIR parent does not exist: $manual_parent"
    [[ ! -e "$MANUAL_INCUS_DIR" ]] \
        || die "INCUS_SMOKE_DIR already exists: $MANUAL_INCUS_DIR"
    if (( ${#MANUAL_INCUS_DIR} + ${#MANUAL_INCUS_DIR_SUFFIX} >= 108 )); then
        die "INCUS_SMOKE_DIR is too long for Incus Unix sockets: $MANUAL_INCUS_DIR"
    fi
    mkdir -m 0700 "$MANUAL_INCUS_DIR"
    export INCUS_DIR="$MANUAL_INCUS_DIR"
    MANUAL_INCUS_LOG="$MANUAL_INCUS_DIR/incusd.log"
    MANUAL_INCUS_STDIO="$MANUAL_INCUS_DIR/incusd.stdio"
    log "starting Incus daemon directly with isolated state at $MANUAL_INCUS_DIR"
    INCUS_USE_SUDO=1
    if [[ "$(id -u)" -eq 0 ]]; then
        env INCUS_DIR="$INCUS_DIR" \
            "$daemon" --group incus-admin --logfile "$MANUAL_INCUS_LOG" \
            >"$MANUAL_INCUS_STDIO" 2>&1 &
    else
        have sudo || die "sudo is required to start the Incus daemon"
        # The runner shell intentionally owns this diagnostic file, not sudo.
        # shellcheck disable=SC2024
        sudo env INCUS_DIR="$INCUS_DIR" \
            "$daemon" --group incus-admin --logfile "$MANUAL_INCUS_LOG" \
            >"$MANUAL_INCUS_STDIO" 2>&1 &
    fi
    MANUAL_INCUS_PID="$!"

    if ! wait_for_incus_client; then
        die "Incus daemon did not become usable after direct startup"
    fi
}

ensure_incus_ready() {
    if incus info >/dev/null 2>&1; then
        INCUS_USE_SUDO=0
    elif sudo_incus_info >/dev/null 2>&1; then
        INCUS_USE_SUDO=1
    elif [[ -n "${INCUS_DIR:-}" ]]; then
        start_manual_incus
    elif have systemctl && systemd_is_pid_one; then
        log "starting Incus socket and daemon through systemd"
        if ! sudo_cmd systemctl is-active --quiet incus.socket; then
            sudo_cmd systemctl start incus.socket
            SYSTEMD_INCUS_SOCKET_STARTED=1
        fi
        if ! sudo_cmd systemctl is-active --quiet incus.service; then
            sudo_cmd systemctl start incus.service
            SYSTEMD_INCUS_SERVICE_STARTED=1
        fi
        wait_for_incus_client || die "Incus daemon did not become usable after systemd startup"
    else
        start_manual_incus
    fi

    if ! incus_has_storage_pool; then
        log "initializing Incus with minimal defaults"
        incus_cmd admin init --minimal
    fi
    incus_has_storage_pool || die "Incus initialization did not create a storage pool"
}

default_storage_pool() {
    local pool
    pool="$(incus_cmd profile device get default root pool 2>/dev/null || true)"
    if [[ -n "$pool" ]]; then
        printf '%s\n' "$pool"
        return
    fi
    pool="$(incus_cmd storage list --format csv | awk -F, 'NF {print $1; exit}' || true)"
    [[ -n "$pool" ]] || die "could not determine an Incus storage pool"
    printf '%s\n' "$pool"
}

default_storage_driver() {
    local pool="$1"
    local driver

    driver="$(incus_cmd storage show "$pool" 2>/dev/null | awk -F': ' '$1 == "driver" {print $2; exit}' || true)"
    [[ -n "$driver" ]] || die "could not determine Incus storage driver for pool $pool"
    printf '%s\n' "$driver"
}

bootstrap_cmd() {
    if [[ "$INCUS_USE_SUDO" == "1" ]]; then
        sudo env PATH="$PATH" HOME="$HOME" INCUS_DIR="${INCUS_DIR:-}" \
            "$repo_root/scripts/incus-bootstrap.sh" "$@"
    else
        "$repo_root/scripts/incus-bootstrap.sh" "$@"
    fi
}

image_alias_exists() {
    incus_cmd image alias list --format csv 2>/dev/null \
        | awk -F, -v name="$image_alias" '$1 == name { found = 1 } END { exit !found }'
}

ensure_smoke_names_available() {
    incus_cmd info "$container_name" >/dev/null 2>&1 \
        && die "smoke container already exists: $container_name"
    incus_cmd info "$bootstrap_container_name" >/dev/null 2>&1 \
        && die "bootstrap smoke container already exists: $bootstrap_container_name"
    incus_cmd profile show "$profile_name" >/dev/null 2>&1 \
        && die "smoke profile already exists: $profile_name"
    incus_cmd profile show "$bootstrap_profile_name" >/dev/null 2>&1 \
        && die "bootstrap smoke profile already exists: $bootstrap_profile_name"
    image_alias_exists && die "smoke image alias already exists: $image_alias"
    return 0
}

ensure_smoke_profile() {
    local pool

    [[ -f "$profile_yaml" ]] || die "missing profile YAML: $profile_yaml"
    pool="$(default_storage_pool)"
    SMOKE_RENDERED_PROFILE="$(mktemp)"
    sed \
        -e "s/^name: .*/name: $profile_name/" \
        -e "s/^    pool: .*/    pool: $pool/" \
        "$profile_yaml" >"$SMOKE_RENDERED_PROFILE"

    log "creating Incus smoke profile $profile_name"
    SMOKE_PROFILE_CREATED=1
    incus_cmd profile create "$profile_name"
    log "applying smoke profile $profile_name with storage pool $pool"
    incus_cmd profile edit "$profile_name" <"$SMOKE_RENDERED_PROFILE"
    rm -f "$SMOKE_RENDERED_PROFILE"
    SMOKE_RENDERED_PROFILE=""
}

wait_for_running() {
    local name="$1"
    local state

    for _ in $(seq 1 90); do
        state="$(incus_cmd info "$name" 2>/dev/null | awk -F': ' '$1 == "Status" {print $2; exit}' || true)"
        if [[ "$state" == "RUNNING" ]]; then
            return
        fi
        sleep 1
    done

    die "$name did not reach RUNNING state"
}

container_file_exists() {
    local name="$1"
    local path="$2"
    local tmp

    tmp="$(mktemp)"
    if incus_cmd file pull "$name$path" "$tmp" >/dev/null 2>&1; then
        rm -f "$tmp"
        return 0
    fi
    rm -f "$tmp"
    return 1
}

assert_container_config() {
    local name="$1"
    local key="$2"
    local expected="$3"
    local actual

    actual="$(incus_cmd config get "$name" "$key")"
    if [[ "$actual" != "$expected" ]]; then
        die "$name config $key was '$actual', expected '$expected'"
    fi
}

if [[ -z "$image_tar" ]]; then
    image_tar="$(find "$export_dir" -maxdepth 1 -type f -name 'labby-incus-*.tar.xz' -print -quit)"
fi
[[ -n "$image_tar" && -f "$image_tar" ]] || die "missing exported image tarball in $export_dir"

install_incus_if_needed
ensure_incus_ready
ensure_smoke_names_available
SMOKE_RESOURCES_OWNED=1
ensure_smoke_profile

log "importing $image_tar as $image_alias"
fingerprint="$(sha256sum "$image_tar" | awk '{print $1}')"
SMOKE_IMAGE_FINGERPRINT="$fingerprint"
if incus_cmd image info "$fingerprint" >/dev/null 2>&1; then
    log "image fingerprint $fingerprint already exists; reusing it"
    incus_cmd image alias create "$image_alias" "$fingerprint"
else
    SMOKE_IMAGE_IMPORTED=1
    incus_cmd image import "$image_tar" --alias "$image_alias"
fi

log "launching $container_name"
incus_cmd init "$image_alias" "$container_name" --profile default --profile "$profile_name"

log "checking stopped image does not contain persisted runtime state"
for path in \
    /home/labby/.labby/.env \
    /root/.labby/.env \
    /run/labby-ts-authkey \
    /var/lib/tailscale/tailscaled.state
do
    if container_file_exists "$container_name" "$path"; then
        echo "forbidden baked runtime state exists: $path" >&2
        exit 1
    fi
done

incus_cmd start "$container_name"
wait_for_running "$container_name"

log "checking baked toolchain"
incus_cmd exec "$container_name" -- su - labby -c 'set -e
node --version
npm --version
uv --version
python --version
rustc --version
cargo --version
go version
mise --version
chezmoi --version
claude --version
codex --version
gemini --version'

log "checking optional Android SDK contract"
if [[ "$expect_android_sdk" == "1" ]]; then
    incus_cmd exec "$container_name" -- sh -lc 'set -e
adb version | head -2'
else
    incus_cmd exec "$container_name" -- sh -lc 'set -e
if command -v adb >/dev/null 2>&1; then
    echo "adb was unexpectedly baked into the default image" >&2
    exit 1
fi'
fi

log "checking root-level tools"
incus_cmd exec "$container_name" -- sh -lc 'set -e
ffmpeg -version | head -1
jq --version
rg --version | head -1
lsof -v 2>&1 | head -1
rsync --version | head -1
tailscale version | head -1
labby --version'

log "checking image does not contain runtime secrets"
# shellcheck disable=SC2016
incus_cmd exec "$container_name" -- sh -lc 'set -eu
for path in \
    /home/labby/.labby/.env \
    /root/.labby/.env \
    /run/labby-ts-authkey
do
    if test -e "$path"; then
        echo "forbidden runtime state exists: $path" >&2
        exit 1
    fi
done
if env | grep -E "^(TS_AUTHKEY|LABBY_MCP_HTTP_TOKEN|OPENAI_API_KEY|ANTHROPIC_API_KEY|GITHUB_TOKEN|GH_TOKEN|NPM_TOKEN|CARGO_REGISTRY_TOKEN)=" >&2; then
    exit 1
fi'

log "checking provision convergence"
incus_cmd exec "$container_name" -- labby setup --provision --yes
incus_cmd exec "$container_name" -- systemctl is-active labby
incus_cmd exec "$container_name" -- curl -fsS --connect-timeout 2 --max-time 10 http://127.0.0.1:8765/ready

log "checking operator bootstrap path"
storage_pool="$(default_storage_pool)"
storage_driver="$(default_storage_driver "$storage_pool")"
bootstrap_cmd \
    --image "$image_alias" \
    --name "$bootstrap_container_name" \
    --profile-name "$bootstrap_profile_name" \
    --profile-file "$profile_yaml" \
    --backup-config "$backup_yaml" \
    --storage-driver "$storage_driver" \
    --storage-pool "$storage_pool" \
    --skip-install
incus_cmd exec "$bootstrap_container_name" -- systemctl is-active labby
incus_cmd exec "$bootstrap_container_name" -- curl -fsS --connect-timeout 2 --max-time 10 http://127.0.0.1:8765/ready
assert_container_config "$bootstrap_container_name" snapshots.schedule "@daily"
assert_container_config "$bootstrap_container_name" snapshots.expiry "14d"
assert_container_config "$bootstrap_container_name" snapshots.pattern "labby-{{ creation_date|date:'2006-01-02_15-04-05' }}"
assert_container_config "$bootstrap_container_name" snapshots.schedule.stopped "false"

log "image smoke test passed"
