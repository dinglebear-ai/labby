#!/usr/bin/env bash
# Focused behavioral tests for the Unraid plugin shell/PHP runtime paths.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
rc_script="$repo_root/unraid/source/usr/local/emhttp/plugins/labby/scripts/rc.labby"
incus_env_script="$repo_root/unraid/source/usr/local/emhttp/plugins/labby/scripts/labby-incus-env.sh"
incus_init_script="$repo_root/unraid/source/usr/local/emhttp/plugins/labby/scripts/labby-incus-init.sh"
page_file="$repo_root/unraid/source/usr/local/emhttp/plugins/labby/Labby.page"
profile_file="$repo_root/unraid/source/usr/local/emhttp/plugins/labby/incus/labby-gateway-profile.yaml"

tmp="$(mktemp -d)"
cleanup() {
    if [ -f "$tmp/pidfile" ]; then
        kill "$(cat "$tmp/pidfile")" >/dev/null 2>&1 || true
    fi
    if [ -f "$tmp/manual-native-pid" ]; then
        kill "$(cat "$tmp/manual-native-pid")" >/dev/null 2>&1 || true
    fi
    rm -rf "$tmp"
}
trap cleanup EXIT

fail() {
    echo "unraid-runtime-tests: $*" >&2
    exit 1
}

assert_file_contains() {
    local file="$1"
    local pattern="$2"
    grep -Fq "$pattern" "$file" || fail "expected $file to contain: $pattern"
}

assert_file_not_contains() {
    local file="$1"
    local pattern="$2"
    ! grep -Fq "$pattern" "$file" || fail "did not expect $file to contain: $pattern"
}

write_cfg() {
    local mode="$1"
    cat > "$tmp/labby.cfg" <<CFG
RUNTIME_MODE="$mode"
INCUS_CONTAINER_NAME="labby-gateway"
INCUS_IMAGE_VERSION="1.2.0"
INCUS_IMAGE_SHA256="dfb57f59b52a84db5b14ac71588b676d7135d4b24916628006aaaed8f022c25d"
INCUS_TS_AUTHKEY=""
INCUS_BRIDGE_NAME="labbybr0"
INCUS_BRIDGE_SUBNET="10.99.99.1/24"
INCUS_EGRESS_POLICY="block-lan"
SERVICE="enabled"
LABBY_DIR="$tmp/labby-state"
HTTP_HOST="127.0.0.1"
HTTP_PORT="8765"
CFG
}

mkdir -p "$tmp/bin" "$tmp/emhttp/bin" "$tmp/emhttp/scripts" "$tmp/incus-prefix/bin" "$tmp/incus-prefix/lib" "$tmp/incus-prefix/libexec/incus"
touch "$tmp/incus-prefix/bin/incus"
chmod +x "$tmp/incus-prefix/bin/incus"

cat > "$tmp/bin/timeout" <<'EOF'
#!/usr/bin/env bash
shift
exec "$@"
EOF
cat > "$tmp/bin/mountpoint" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
cat > "$tmp/bin/curl" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
cat > "$tmp/bin/iptables" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${IPTABLES_LOG:?}"
case "$1" in
    -C) exit 1 ;;
    *) exit 0 ;;
esac
EOF
cat > "$tmp/bin/ip" <<'EOF'
#!/usr/bin/env bash
if [ "$1" = "-4" ] && [ "$2" = "route" ] && [ "$3" = "show" ]; then
    cat "${IP_ROUTE_OUTPUT:?}"
    exit 0
fi
exit 1
EOF
cat > "$tmp/bin/incus" <<'EOF'
#!/usr/bin/env bash
state_file="${INCUS_STATE_FILE:?}"
state="$(cat "$state_file" 2>/dev/null || printf 'missing')"
case "$1" in
    info)
        case "$state" in
            missing)
                echo "Error: Instance not found" >&2
                exit 1
                ;;
            query-fail)
                echo "Error: socket unavailable" >&2
                exit 1
                ;;
            running) echo "Status: Running" ;;
            stopped) echo "Status: Stopped" ;;
            *) echo "Status: $state" ;;
        esac
        ;;
    stop)
        [ "$state" = "stop-fail" ] && exit 1
        printf 'stopped\n' > "$state_file"
        ;;
    exec)
        [ "$state" = "running" ] && exit 0
        exit 1
        ;;
    network)
        case "$2" in
            show)
                case "${INCUS_NETWORK_MANAGED:-true}" in
                    true) echo "managed: true" ;;
                    false) echo "managed: false" ;;
                    missing) exit 1 ;;
                esac
                ;;
            get)
                case "$4" in
                    ipv4.address) printf '%s\n' "${INCUS_NETWORK_IPV4_ADDRESS:-10.99.99.1/24}" ;;
                    ipv4.nat) printf '%s\n' "${INCUS_NETWORK_IPV4_NAT:-true}" ;;
                    ipv6.address) printf '%s\n' "${INCUS_NETWORK_IPV6_ADDRESS:-none}" ;;
                    ipv6.nat) printf '%s\n' "${INCUS_NETWORK_IPV6_NAT:-false}" ;;
                    *) exit 1 ;;
                esac
                ;;
            create) exit 0 ;;
        esac
        ;;
    *) exit 0 ;;
esac
EOF
chmod +x "$tmp/bin/timeout" "$tmp/bin/mountpoint" "$tmp/bin/curl" "$tmp/bin/iptables" "$tmp/bin/ip" "$tmp/bin/incus"

cat > "$tmp/emhttp/bin/labby" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${LABBY_FAKE_BIN_LOG:?}"
while :; do sleep 60; done
EOF
cat > "$tmp/emhttp/scripts/labby-preflight.sh" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
cat > "$tmp/emhttp/scripts/labby-incus-env.sh" <<'EOF'
#!/usr/bin/env bash
export INCUS="${INCUS:?}"
EOF
cat > "$tmp/emhttp/scripts/labby-incus-init.sh" <<'EOF'
#!/usr/bin/env bash
printf 'ran\n' >> "${INIT_LOG:?}"
exit 0
EOF
chmod +x "$tmp/emhttp/bin/labby" "$tmp/emhttp/scripts/labby-preflight.sh" "$tmp/emhttp/scripts/labby-incus-env.sh" "$tmp/emhttp/scripts/labby-incus-init.sh"

run_rc() {
    env \
        EMHTTP="$tmp/emhttp" \
        BIN="$tmp/emhttp/bin/labby" \
        CFG="$tmp/labby.cfg" \
        INCUS="$tmp/bin/incus" \
        TIMEOUT="$tmp/bin/timeout" \
        PIDFILE="$tmp/pidfile" \
        LOG="$tmp/labby.log" \
        IPTABLES_LOG="$tmp/iptables.log" \
        IP_ROUTE_OUTPUT="$tmp/ip-routes" \
        INCUS_STATE_FILE="$tmp/incus-state" \
        LABBY_FAKE_BIN_LOG="$tmp/fake-labby.log" \
        INIT_LOG="$tmp/init.log" \
        PATH="$tmp/bin:$PATH" \
        "$rc_script" "$@"
}

test_env_sourcer_is_idempotent() {
    local count

    count="$(
        INCUS_PREFIX="$tmp/incus-prefix" PATH="/usr/bin" LD_LIBRARY_PATH="/base" bash -c "
            set -euo pipefail
            . '$incus_env_script'
            . '$incus_env_script'
            printf '%s\n' \"\$PATH\" | tr ':' '\n' | grep -Fx '$tmp/incus-prefix/bin' | wc -l
            printf '%s\n' \"\$PATH\" | tr ':' '\n' | grep -Fx '$tmp/incus-prefix/libexec/incus' | wc -l
            printf '%s\n' \"\$LD_LIBRARY_PATH\" | tr ':' '\n' | grep -Fx '$tmp/incus-prefix/lib' | wc -l
        "
    )"
    [ "$count" = $'1\n1\n1' ] || fail "incus env sourcer duplicated PATH/LD_LIBRARY_PATH entries: $count"
}

test_native_start_does_not_require_incus() {
    write_cfg native
    printf 'missing\n' > "$tmp/incus-state"
    mv "$tmp/bin/incus" "$tmp/bin/incus.real"
    run_rc start > "$tmp/native-start.out"
    assert_file_contains "$tmp/native-start.out" "labby: ready"
    assert_file_not_contains "$tmp/native-start.out" "incus-unraid"
    run_rc stop >/dev/null
    mv "$tmp/bin/incus.real" "$tmp/bin/incus"
}

test_native_start_ignores_unproven_incus_query_failure() {
    write_cfg native
    rm -f "$tmp/labby-state/.labby-incus-runtime-created"
    printf 'query-fail\n' > "$tmp/incus-state"
    run_rc start > "$tmp/native-query-fail.out"
    assert_file_contains "$tmp/native-query-fail.out" "no Labby Incus runtime marker exists"
    assert_file_contains "$tmp/native-query-fail.out" "labby: ready"
    run_rc stop >/dev/null
}

test_native_start_fails_closed_with_incus_marker_and_missing_cli() {
    write_cfg native
    mkdir -p "$tmp/labby-state"
    : > "$tmp/labby-state/.labby-incus-runtime-created"
    mv "$tmp/bin/incus" "$tmp/bin/incus.real"
    if run_rc start > "$tmp/native-marker-missing-cli.out" 2>&1; then
        fail "native start succeeded even though an Incus runtime marker existed and the Incus CLI was missing"
    fi
    mv "$tmp/bin/incus.real" "$tmp/bin/incus"
    assert_file_contains "$tmp/native-marker-missing-cli.out" "Incus runtime marker exists"
    rm -f "$tmp/labby-state/.labby-incus-runtime-created"
}

test_native_to_incus_stops_native_pid() {
    write_cfg incus
    printf 'missing\n' > "$tmp/incus-state"
    sleep 600 &
    native_pid="$!"
    disown "$native_pid" 2>/dev/null || true
    printf '%s\n' "$native_pid" > "$tmp/pidfile"
    printf '%s\n' "$native_pid" > "$tmp/manual-native-pid"

    run_rc start > "$tmp/incus-start.out"
    assert_file_contains "$tmp/incus-start.out" "stopping native runtime before starting Incus mode"
    assert_file_contains "$tmp/init.log" "ran"
    if kill -0 "$(cat "$tmp/manual-native-pid")" 2>/dev/null; then
        fail "native pid survived incus mode switch"
    fi
    rm -f "$tmp/manual-native-pid" "$tmp/pidfile" "$tmp/init.log"
}

test_incus_to_native_stops_incus_first() {
    write_cfg native
    printf 'running\n' > "$tmp/incus-state"
    run_rc start > "$tmp/incus-to-native.out"
    assert_file_contains "$tmp/incus-to-native.out" "stopping Incus runtime before starting native mode"
    [ "$(cat "$tmp/incus-state")" = "stopped" ] || fail "incus state was not stopped before native start"
    run_rc stop >/dev/null
}

test_incus_query_failure_is_not_stopped() {
    write_cfg incus
    printf 'query-fail\n' > "$tmp/incus-state"
    if run_rc stop > "$tmp/query-fail.out" 2>&1; then
        fail "incus query failure returned success"
    fi
    assert_file_contains "$tmp/query-fail.out" "failed to query Incus"
}

test_incus_stop_rejects_unsafe_state() {
    write_cfg incus
    printf 'frozen\n' > "$tmp/incus-state"
    if run_rc stop > "$tmp/frozen-stop.out" 2>&1; then
        fail "frozen Incus state returned stop success"
    fi
    assert_file_contains "$tmp/frozen-stop.out" "FROZEN"
    assert_file_contains "$tmp/frozen-stop.out" "not safely stopped"
}

test_incus_status_preserves_stopped_and_unsafe_states() {
    write_cfg incus
    printf 'stopped\n' > "$tmp/incus-state"
    run_rc status > "$tmp/stopped-status.out"
    assert_file_contains "$tmp/stopped-status.out" "STOPPED (incus container labby-gateway)"

    printf 'error\n' > "$tmp/incus-state"
    if run_rc status > "$tmp/error-status.out" 2>&1; then
        fail "ERROR Incus state returned status success"
    fi
    assert_file_contains "$tmp/error-status.out" "ERROR (incus container labby-gateway; not safe to treat as stopped)"
}

source_init_library() {
    export EMHTTP="$repo_root/unraid/source/usr/local/emhttp/plugins/labby"
    export CFG="$tmp/labby.cfg"
    export INCUS_PREFIX="$tmp/incus-prefix"
    export INCUS="$tmp/bin/incus"
    export INCUS_STATE_FILE="$tmp/incus-state"
    export IP_ROUTE_OUTPUT="$tmp/ip-routes"
    export PATH="$tmp/bin:$PATH"
    export LABBY_INCUS_INIT_LIBRARY=1
    # shellcheck disable=SC1090
    . "$incus_init_script"
}

test_incus_init_instance_query_failures_are_fatal() {
    write_cfg incus
    : > "$tmp/ip-routes"
    printf 'query-fail\n' > "$tmp/incus-state"
    set +e
    (
        set -euo pipefail
        source_init_library
        instance_exists
    ) > "$tmp/instance-query-fail.out" 2>&1
    status=$?
    set -e
    [ "$status" -ne 0 ] || fail "instance query failure returned success"
    assert_file_contains "$tmp/instance-query-fail.out" "failed to query Incus instance"
}

test_bridge_collision_checks_whole_cidr() {
    write_cfg incus
    printf 'missing\n' > "$tmp/incus-state"
    printf '10.99.99.128/25 dev br0\n' > "$tmp/ip-routes"
    set +e
    (
        set -euo pipefail
        source_init_library
        validate_bridge_subnet_collision
    ) > "$tmp/bridge-overlap.out" 2>&1
    status=$?
    set -e
    [ "$status" -ne 0 ] || fail "overlapping route was not rejected"
    assert_file_contains "$tmp/bridge-overlap.out" "collides with existing route"

    printf '10.99.99.0/24 dev labbybr0\n' > "$tmp/ip-routes"
    (
        set -euo pipefail
        source_init_library
        validate_bridge_subnet_collision
    )
}

test_managed_bridge_validates_full_posture() {
    write_cfg incus
    : > "$tmp/ip-routes"
    if (
        set -euo pipefail
        INCUS_NETWORK_IPV4_NAT=false
        export INCUS_NETWORK_IPV4_NAT
        source_init_library
        ensure_bridge
    ) > "$tmp/bridge-posture.out" 2>&1; then
        fail "managed bridge with wrong ipv4.nat was accepted"
    fi
    assert_file_contains "$tmp/bridge-posture.out" "ipv4.nat=false, expected true"

    (
        set -euo pipefail
        INCUS_NETWORK_IPV4_ADDRESS="10.99.99.1/24"
        INCUS_NETWORK_IPV4_NAT=true
        INCUS_NETWORK_IPV6_ADDRESS=none
        INCUS_NETWORK_IPV6_NAT=false
        export INCUS_NETWORK_IPV4_ADDRESS INCUS_NETWORK_IPV4_NAT INCUS_NETWORK_IPV6_ADDRESS INCUS_NETWORK_IPV6_NAT
        source_init_library
        ensure_bridge
    )
}

test_labby_dir_validator_rejects_non_array_paths() {
    write_cfg incus
    # shellcheck disable=SC2030,SC2031
    (
        set -euo pipefail
        source_init_library
        export LABBY_DIR="/mnt/disk1/appdata/labby"
        validate_array_backed_labby_dir
        export LABBY_DIR="/mnt/cache/appdata/labby"
        validate_array_backed_labby_dir
    )
    if (
        set -euo pipefail
        source_init_library
        # shellcheck disable=SC2030,SC2031
        export LABBY_DIR="/tmp/labby"
        validate_array_backed_labby_dir
    ) > "$tmp/labby-dir-tmp.out" 2>&1; then
        fail "LABBY_DIR validator accepted /tmp/labby"
    fi
    if (
        set -euo pipefail
        source_init_library
        # shellcheck disable=SC2030,SC2031
        export LABBY_DIR="/mnt/disk1foo/labby"
        validate_array_backed_labby_dir
    ) > "$tmp/labby-dir-diskfoo.out" 2>&1; then
        fail "LABBY_DIR validator accepted /mnt/disk1foo/labby"
    fi
    assert_file_contains "$tmp/labby-dir-tmp.out" "got: /tmp/labby"
    assert_file_contains "$tmp/labby-dir-diskfoo.out" "got: /mnt/disk1foo/labby"
}

test_profile_is_rendered_in_one_edit() {
    assert_file_contains "$profile_file" "  eth0:"
    assert_file_contains "$profile_file" "    network: labbybr0"
    ! grep -Fq "profile device add" "$incus_init_script" \
        || fail "labby-incus-init.sh still adds eth0 outside profile edit"
    ! grep -Fq "profile device remove" "$incus_init_script" \
        || fail "labby-incus-init.sh still removes eth0 outside profile edit"
}

test_tailscale_key_redaction_helpers() {
    php <<PHP
<?php
\$page = file_get_contents('$page_file');
if (!preg_match('/function labby_redact_ts_authkey\\(.*?\\n}\\n/s', \$page, \$m)) {
    fwrite(STDERR, "labby_redact_ts_authkey function not found\n");
    exit(1);
}
eval(\$m[0]);
\$input = "INCUS_TS_AUTHKEY=\"tskey-secret\" # secret comment\nSERVICE=\"enabled\"\n";
\$out = labby_redact_ts_authkey(\$input);
if (str_contains(\$out, 'tskey-secret') || !str_contains(\$out, 'INCUS_TS_AUTHKEY="" # secret comment')) {
    fwrite(STDERR, "Labby.page redaction helper did not redact as expected\n");
    exit(1);
}
if (!str_contains(\$page, 'INCUS_IMAGE_SHA256 is required when RUNTIME_MODE is incus.')) {
    fwrite(STDERR, "Incus-mode SHA validation guard missing\n");
    exit(1);
}
if (!str_contains(\$page, 'function labby_is_array_backed_path')) {
    fwrite(STDERR, "LABBY_DIR array/cache path validation helper missing\n");
    exit(1);
}
if (!str_contains(\$page, 'starting with a lowercase letter')) {
    fwrite(STDERR, "Incus instance name lowercase-letter validation message missing\n");
    exit(1);
}
PHP

    cat > "$tmp/labby.cfg" <<CFG
RUNTIME_MODE="incus"
INCUS_CONTAINER_NAME="labby-gateway"
INCUS_IMAGE_VERSION="1.2.0"
INCUS_IMAGE_SHA256="dfb57f59b52a84db5b14ac71588b676d7135d4b24916628006aaaed8f022c25d"
INCUS_TS_AUTHKEY="tskey-secret"                   # incus mode only - write-only one-shot Tailscale auth key
INCUS_BRIDGE_NAME="labbybr0"
INCUS_BRIDGE_SUBNET="10.99.99.1/24"
INCUS_EGRESS_POLICY="block-lan"
LABBY_DIR="$tmp/labby-state"
CFG
    (
        set -euo pipefail
        source_init_library
        clear_stored_ts_authkey
    )
    assert_file_not_contains "$tmp/labby.cfg" "tskey-secret"
    assert_file_not_contains "$tmp/labby.cfg.bak" "tskey-secret"
    assert_file_contains "$tmp/labby.cfg" 'INCUS_TS_AUTHKEY=""'
    assert_file_contains "$tmp/labby.cfg.bak" 'INCUS_TS_AUTHKEY=""'
}

test_page_embeds_gateway_admin_ui() {
    assert_file_contains "$page_file" '<iframe'
    assert_file_contains "$page_file" 'title="Labby Gateway Admin"'
    assert_file_contains "$page_file" 'Manage the gateway below'
    assert_file_not_contains "$page_file" 'thin settings/status shell that links out'
    assert_file_not_contains "$page_file" 'links out to labby'
}

test_env_sourcer_is_idempotent
test_native_start_does_not_require_incus
test_native_start_ignores_unproven_incus_query_failure
test_native_start_fails_closed_with_incus_marker_and_missing_cli
test_native_to_incus_stops_native_pid
test_incus_to_native_stops_incus_first
test_incus_query_failure_is_not_stopped
test_incus_stop_rejects_unsafe_state
test_incus_status_preserves_stopped_and_unsafe_states
test_incus_init_instance_query_failures_are_fatal
test_bridge_collision_checks_whole_cidr
test_managed_bridge_validates_full_posture
test_labby_dir_validator_rejects_non_array_paths
test_profile_is_rendered_in_one_edit
test_tailscale_key_redaction_helpers
test_page_embeds_gateway_admin_ui

echo "unraid runtime behavior tests OK"
