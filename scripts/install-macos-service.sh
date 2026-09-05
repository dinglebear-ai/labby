#!/usr/bin/env bash
set -euo pipefail

service_label="${LABBY_LAUNCHD_LABEL:-ai.dinglebear.labby}"
service_domain="gui/$(id -u)"
service_dir="${HOME}/Library/LaunchAgents"
plist_path="${service_dir}/${service_label}.plist"
binary_path="${LABBY_SERVICE_BIN:-${HOME}/.local/bin/labby}"
service_host="${LABBY_SERVICE_HOST:-127.0.0.1}"
service_port="${LABBY_SERVICE_PORT:-8765}"
labby_home="${LABBY_HOME:-${HOME}/.labby}"
state_dir="${LABBY_STATE_DIR:-${labby_home}}"

usage() {
    cat <<'EOF'
Usage: scripts/install-macos-service.sh <install|restart|status|uninstall>

Install or manage the per-user macOS Labby LaunchAgent.

Environment overrides:
  LABBY_LAUNCHD_LABEL  LaunchAgent label (default: ai.dinglebear.labby)
  LABBY_SERVICE_BIN    Labby executable (default: ~/.local/bin/labby)
  LABBY_SERVICE_HOST   Bind host (default: 127.0.0.1)
  LABBY_SERVICE_PORT   Bind port (default: 8765)
  LABBY_HOME           Stable config/state/working directory (default: ~/.labby)
  LABBY_STATE_DIR      Log directory (default: ~/.labby)
EOF
}

require_macos() {
    if [[ "$(uname -s)" != "Darwin" ]]; then
        echo "error: the macOS Labby service requires Darwin" >&2
        exit 1
    fi
}

validate_service_paths() {
    if [[ "$binary_path" != /* ]]; then
        echo "error: LABBY_SERVICE_BIN must be an absolute path: $binary_path" >&2
        exit 1
    fi
    if [[ "$state_dir" != /* ]]; then
        echo "error: LABBY_STATE_DIR must be an absolute path: $state_dir" >&2
        exit 1
    fi
    if [[ "$labby_home" != /* ]]; then
        echo "error: LABBY_HOME must be an absolute path: $labby_home" >&2
        exit 1
    fi
}

xml_escape() {
    # Bash changed the meaning of `&` in parameter-substitution replacements;
    # sed keeps this identical on macOS Bash 3.2 and current Linux Bash.
    printf '%s' "$1" | sed \
        -e 's/&/\&amp;/g' \
        -e 's/</\&lt;/g' \
        -e 's/>/\&gt;/g' \
        -e 's/"/\&quot;/g' \
        -e "s/'/\\&apos;/g"
}

write_plist() {
    local temp_path
    local escaped_binary
    local escaped_working_dir
    local escaped_log_dir
    local escaped_label
    local escaped_host
    local escaped_port

    escaped_binary=$(xml_escape "$binary_path")
    escaped_working_dir=$(xml_escape "$labby_home")
    escaped_log_dir=$(xml_escape "$state_dir")
    escaped_label=$(xml_escape "$service_label")
    escaped_host=$(xml_escape "$service_host")
    escaped_port=$(xml_escape "$service_port")
    mkdir -p "$service_dir" "$labby_home" "$state_dir"
    temp_path=$(mktemp "${plist_path}.tmp.XXXXXX")
    trap 'rm -f "$temp_path"' RETURN

    cat >"$temp_path" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${escaped_label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>${escaped_binary}</string>
        <string>serve</string>
        <string>--host</string>
        <string>${escaped_host}</string>
        <string>--port</string>
        <string>${escaped_port}</string>
        <string>--log-level</string>
        <string>info</string>
    </array>
    <key>WorkingDirectory</key>
    <string>${escaped_working_dir}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>LABBY_HOME</key>
        <string>${escaped_working_dir}</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>5</integer>
    <key>StandardOutPath</key>
    <string>${escaped_log_dir}/serve.log</string>
    <key>StandardErrorPath</key>
    <string>${escaped_log_dir}/serve.error.log</string>
</dict>
</plist>
EOF

    plutil -lint "$temp_path" >/dev/null
    install -m 644 "$temp_path" "$plist_path"
    rm -f "$temp_path"
    trap - RETURN
}

stop_loaded_service() {
    if launchctl print "${service_domain}/${service_label}" >/dev/null 2>&1; then
        launchctl bootout "${service_domain}/${service_label}"
        return
    else
        local status=$?
        # launchctl uses EX_NOTFOUND (113) when the service is not loaded. Only
        # that idempotent absence is safe to suppress; all other launchd failures
        # must remain visible to the operator.
        if [[ "$status" -ne 113 ]]; then
            return "$status"
        fi
    fi
}

wait_for_health() {
    local health_url="http://${service_host}:${service_port}/health"
    local attempts="${LABBY_TEST_HEALTH_ATTEMPTS:-30}"
    local _
    for ((_=0; _<attempts; _++)); do
        if curl --fail --silent --show-error --max-time 2 "$health_url" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    echo "error: Labby did not become healthy at ${health_url}" >&2
    return 1
}

restore_install_state() {
    local previous_plist=$1
    local was_loaded=$2
    local rollback_failed=0

    if launchctl print "${service_domain}/${service_label}" >/dev/null 2>&1; then
        launchctl bootout "${service_domain}/${service_label}" || rollback_failed=1
    fi
    if [[ -n "$previous_plist" ]]; then
        install -m 644 "$previous_plist" "$plist_path" || rollback_failed=1
    else
        rm -f "$plist_path" || rollback_failed=1
    fi
    if [[ "$was_loaded" -eq 1 ]]; then
        launchctl bootstrap "$service_domain" "$plist_path" || rollback_failed=1
        launchctl kickstart -k "${service_domain}/${service_label}" || rollback_failed=1
    fi
    return "$rollback_failed"
}

install_service() {
    if [[ ! -x "$binary_path" ]]; then
        echo "error: Labby binary not found or not executable: $binary_path" >&2
        echo "run 'just macos-service-install' after building the repository" >&2
        exit 1
    fi

    local previous_plist=""
    local was_loaded=0
    local status
    if [[ -f "$plist_path" ]]; then
        previous_plist=$(mktemp "${plist_path}.backup.XXXXXX")
        cp -p "$plist_path" "$previous_plist"
    fi
    if launchctl print "${service_domain}/${service_label}" >/dev/null 2>&1; then
        was_loaded=1
    else
        status=$?
        if [[ "$status" -ne 113 ]]; then
            rm -f "$previous_plist"
            return "$status"
        fi
    fi

    stop_loaded_service || { status=$?; rm -f "$previous_plist"; return "$status"; }
    write_plist || {
        status=$?
        restore_install_state "$previous_plist" "$was_loaded" || echo "error: failed to restore prior macOS service state" >&2
        rm -f "$previous_plist"
        return "$status"
    }
    launchctl bootstrap "$service_domain" "$plist_path" || {
        status=$?
        restore_install_state "$previous_plist" "$was_loaded" || echo "error: failed to restore prior macOS service state" >&2
        rm -f "$previous_plist"
        return "$status"
    }
    launchctl kickstart -k "${service_domain}/${service_label}" || {
        status=$?
        restore_install_state "$previous_plist" "$was_loaded" || echo "error: failed to restore prior macOS service state" >&2
        rm -f "$previous_plist"
        return "$status"
    }
    wait_for_health || {
        status=$?
        restore_install_state "$previous_plist" "$was_loaded" || echo "error: failed to restore prior macOS service state" >&2
        rm -f "$previous_plist"
        return "$status"
    }
    rm -f "$previous_plist"
    echo "Labby macOS service is running: ${service_domain}/${service_label}"
    echo "health: http://${service_host}:${service_port}/health"
}

restart_service() {
    if [[ ! -f "$plist_path" ]]; then
        echo "error: LaunchAgent is not installed: $plist_path" >&2
        echo "run 'just macos-service-install' first" >&2
        exit 1
    fi

    if launchctl print "${service_domain}/${service_label}" >/dev/null 2>&1; then
        :
    else
        local status=$?
        if [[ "$status" -ne 113 ]]; then
            return "$status"
        fi
        launchctl bootstrap "$service_domain" "$plist_path"
    fi
    launchctl kickstart -k "${service_domain}/${service_label}"
    wait_for_health
    echo "Labby macOS service restarted: ${service_domain}/${service_label}"
}

status_service() {
    if launchctl print "${service_domain}/${service_label}"; then
        return 0
    else
        local status=$?
        if [[ "$status" -ne 113 ]]; then
            return "$status"
        fi
        echo "Labby macOS service is not loaded: ${service_domain}/${service_label}" >&2
        return 1
    fi
}

uninstall_service() {
    stop_loaded_service
    rm -f "$plist_path"
    echo "Labby macOS service removed: ${service_domain}/${service_label}"
}

require_macos
validate_service_paths

case "${1:-}" in
    install) install_service ;;
    restart) restart_service ;;
    status) status_service ;;
    uninstall) uninstall_service ;;
    -h|--help) usage ;;
    *) usage >&2; exit 2 ;;
esac
