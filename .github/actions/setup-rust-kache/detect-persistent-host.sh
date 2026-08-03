#!/usr/bin/env bash
set -euo pipefail

managed=false
binary="${KACHE_MANAGED_BINARY:-}"
config="${XDG_CONFIG_HOME:-$HOME/.config}/kache/config.toml"
if command -v systemctl >/dev/null 2>&1 &&
   systemctl --user is-active --quiet kache.service; then
  if [ -z "$binary" ]; then
    pid="$(systemctl --user show kache.service -p MainPID --value 2>/dev/null || true)"
    case "$pid" in
      ''|0|*[!0-9]*) ;;
      *) binary="$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)" ;;
    esac
  fi
  if [ -x "$binary" ] &&
     [ -s "$config" ] &&
     grep -Eq '^[[:space:]]*type[[:space:]]*=[[:space:]]*"s3"' "$config" &&
     grep -Eq '^[[:space:]]*prefix[[:space:]]*=[[:space:]]*"rust"' "$config"; then
    status="$("$binary" daemon status 2>&1 || true)"
    if grep -q 'Daemon:' <<<"$status" && ! grep -q 'not running' <<<"$status"; then
      managed=true
    fi
  fi
fi

printf 'managed=%s\n' "$managed" >>"${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
printf 'binary=%s\n' "$binary" >>"$GITHUB_OUTPUT"
