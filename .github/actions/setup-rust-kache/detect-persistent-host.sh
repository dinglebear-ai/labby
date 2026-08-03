#!/usr/bin/env bash
set -euo pipefail

managed=false
config="${XDG_CONFIG_HOME:-$HOME/.config}/kache/config.toml"
if command -v kache >/dev/null 2>&1 &&
   command -v systemctl >/dev/null 2>&1 &&
   systemctl --user is-active --quiet kache.service &&
   [ -s "$config" ] &&
   grep -Eq '^[[:space:]]*type[[:space:]]*=[[:space:]]*"s3"' "$config" &&
   grep -Eq '^[[:space:]]*prefix[[:space:]]*=[[:space:]]*"rust"' "$config"; then
  status="$(kache daemon status 2>&1 || true)"
  if grep -q 'Daemon:' <<<"$status" && ! grep -q 'not running' <<<"$status"; then
    managed=true
  fi
fi

printf 'managed=%s\n' "$managed" >>"${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
