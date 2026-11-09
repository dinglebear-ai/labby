#!/usr/bin/env bash
set -euo pipefail

if (( $# != 1 )) || [[ ! -d "$1" ]]; then
    printf 'Usage: %s <existing-private-directory>\n' "$0" >&2
    exit 64
fi

tool_dir=$1
version=0.19.8
asset_id=431892850
sha256=70e769ae3872e34d45132b17040859175e11401dc12dddb0303e0b8c7d088f3f
archive="$tool_dir/cargo-deny.tar.gz"
release=https://github.com/EmbarkStudios/cargo-deny/releases/download
asset_api=https://api.github.com/repos/EmbarkStudios/cargo-deny/releases/assets
curl_args=(
    --fail --show-error --silent --location --proto '=https' --tlsv1.2
    --connect-timeout 10 --max-time 45 --retry 5 --retry-max-time 90
    --retry-all-errors --retry-delay 2 --output "$archive"
)

if ! curl "${curl_args[@]}" "$release/$version/cargo-deny-$version-x86_64-unknown-linux-musl.tar.gz"; then
    printf 'Cargo Deny release download failed. Trying the pinned release asset API.\n' >&2
    curl "${curl_args[@]}" \
        -H 'Accept: application/octet-stream' \
        -H 'X-GitHub-Api-Version: 2022-11-28' \
        "$asset_api/$asset_id"
fi

printf '%s  %s\n' "$sha256" "$archive" | sha256sum --check --strict
tar -xzf "$archive" -C "$tool_dir" --strip-components=1
