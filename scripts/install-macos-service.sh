#!/usr/bin/env bash
set -euo pipefail

service_label="${LABBY_LAUNCHD_LABEL:-ai.dinglebear.labby}"
service_domain="gui/$(id -u)"
service_dir="${HOME}/Library/LaunchAgents"
plist_path="${service_dir}/${service_label}.plist"
binary_path="${LABBY_SERVICE_BIN:-${HOME}/.local/bin/labby}"
service_host="${LABBY_SERVICE_HOST:-127.0.0.1}"
service_port="${LABBY_SERVICE_PORT:-8765}"
state_dir="${LABBY_STATE_DIR:-${HOME}/.labby}"

usage() {
    cat <<'EOF'
Usage: scripts/install-macos-service.sh <install|restart|status|uninstall>

Install or manage the per-user macOS Labby LaunchAgent.

Environment overrides:
  LABBY_LAUNCHD_LABEL  LaunchAgent label (default: ai.dinglebear.labby)
  LABBY_SERVICE_BIN    Labby executable (default: ~/.local/bin/labby)
  LABBY_SERVICE_HOST   Bind host (default: 127.0.0.1)
  LABBY_SERVICE_PORT   Bind port (default: 8765)
  LABBY_STATE_DIR      Log directory (default: ~/.labby)
EOF
}

require_macos() {
    if [[ "$(uname -s)" != "Darwin" ]]; then
        echo "error: the macOS Labby service requires Darwin" >&2
        exit 1
    fi
}

xml_escape() {
    local value="$1"
    value=${value//&/&amp;}
    value=${value//</&lt;}
    value=${value//>/&gt;}
    value=${value//\"/&quot;}
    value=${value//\'/&apos;}
    printf '%s' "$value"
}

write_plist() {
    local temp_path
    local escaped_binary
    local escaped_working_dir
    local escaped_log_dir

    escaped_binary=$(xml_escape "$binary_path")
    escaped_working_dir=$(xml_escape "$(pwd)")
    escaped_log_dir=$(xml_escape "$state_dir")
    mkdir -p "$service_dir" "$state_dir"
    temp_path=$(mktemp "${plist_path}.tmp.XXXXXX")
    trap 'rm -f "$temp_path"' RETURN

    cat >"$temp_path" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${service_label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>${escaped_binary}</string>
        <string>serve</string>
        <string>--host</string>
        <string>${service_host}</string>
        <string>--port</string>
        <string>${service_port}</string>
        <string>--log-level</string>
        <string>info</string>
    </array>
    <key>WorkingDirectory</key>
    <string>${escaped_working_dir}</string>
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
    launchctl bootout "${service_domain}/${service_label}" 2>/dev/null || true
}

wait_for_health() {
    local health_url="http://${service_host}:${service_port}/health"
    local attempt
    for attempt in {1..30}; do
        if curl --fail --silent --show-error --max-time 2 "$health_url" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    echo "error: Labby did not become healthy at ${health_url}" >&2
    return 1
}

install_service() {
    if [[ ! -x "$binary_path" ]]; then
        echo "error: Labby binary not found or not executable: $binary_path" >&2
        echo "run 'just macos-service-install' after building the repository" >&2
        exit 1
    fi

    write_plist
    stop_loaded_service
    launchctl bootstrap "$service_domain" "$plist_path"
    launchctl kickstart -k "${service_domain}/${service_label}"
    wait_for_health
    echo "Labby macOS service is running: ${service_domain}/${service_label}"
    echo "health: http://${service_host}:${service_port}/health"
}

restart_service() {
    if [[ ! -f "$plist_path" ]]; then
        echo "error: LaunchAgent is not installed: $plist_path" >&2
        echo "run 'just macos-service-install' first" >&2
        exit 1
    fi

    if ! launchctl print "${service_domain}/${service_label}" >/dev/null 2>&1; then
        launchctl bootstrap "$service_domain" "$plist_path"
    fi
    launchctl kickstart -k "${service_domain}/${service_label}"
    wait_for_health
    echo "Labby macOS service restarted: ${service_domain}/${service_label}"
}

status_service() {
    if ! launchctl print "${service_domain}/${service_label}"; then
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

case "${1:-}" in
    install) install_service ;;
    restart) restart_service ;;
    status) status_service ;;
    uninstall) uninstall_service ;;
    -h|--help) usage ;;
    *) usage >&2; exit 2 ;;
esac
