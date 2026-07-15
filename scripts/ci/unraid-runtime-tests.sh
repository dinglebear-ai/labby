#!/usr/bin/env bash
# Focused behavioral tests for the Unraid plugin shell/PHP runtime paths.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
rc_script="$repo_root/unraid/source/usr/local/emhttp/plugins/labby/scripts/rc.labby"
incus_env_script="$repo_root/unraid/source/usr/local/emhttp/plugins/labby/scripts/labby-incus-env.sh"
incus_init_script="$repo_root/unraid/source/usr/local/emhttp/plugins/labby/scripts/labby-incus-init.sh"
page_file="$repo_root/unraid/source/usr/local/emhttp/plugins/labby/Labby.page"

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
    *) exit 0 ;;
esac
EOF
chmod +x "$tmp/bin/timeout" "$tmp/bin/mountpoint" "$tmp/bin/curl" "$tmp/bin/iptables" "$tmp/bin/incus"

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
    set +e
    run_rc stop > "$tmp/query-fail.out" 2>&1
    status=$?
    set -e
    [ "$status" -ne 0 ] || fail "incus query failure returned success"
    assert_file_contains "$tmp/query-fail.out" "failed to query Incus"
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
        export EMHTTP="$repo_root/unraid/source/usr/local/emhttp/plugins/labby"
        export CFG="$tmp/labby.cfg"
        export INCUS_PREFIX="$tmp/incus-prefix"
        export LABBY_INCUS_INIT_LIBRARY=1
        # shellcheck disable=SC1090
        . "$incus_init_script"
        clear_stored_ts_authkey
    )
    assert_file_not_contains "$tmp/labby.cfg" "tskey-secret"
    assert_file_not_contains "$tmp/labby.cfg.bak" "tskey-secret"
    assert_file_contains "$tmp/labby.cfg" 'INCUS_TS_AUTHKEY=""'
    assert_file_contains "$tmp/labby.cfg.bak" 'INCUS_TS_AUTHKEY=""'
}

test_env_sourcer_is_idempotent
test_native_start_does_not_require_incus
test_native_to_incus_stops_native_pid
test_incus_to_native_stops_incus_first
test_incus_query_failure_is_not_stopped
test_tailscale_key_redaction_helpers

echo "unraid runtime behavior tests OK"
