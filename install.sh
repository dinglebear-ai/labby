#!/bin/sh
# Compatibility entrypoint for the canonical installer in scripts/install.sh.
set -eu

script_dir="$(CDPATH= cd -- "$(dirname -- "$0")" 2>/dev/null && pwd)"
if [ -f "$script_dir/scripts/install.sh" ]; then
    exec "$script_dir/scripts/install.sh" "$@"
fi

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
curl -fsSL --retry 3 \
    https://raw.githubusercontent.com/dinglebear-ai/labby/main/scripts/install.sh \
    -o "$tmp"
sh "$tmp" "$@"
