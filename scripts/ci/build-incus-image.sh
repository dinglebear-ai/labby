#!/usr/bin/env bash
set -euo pipefail

version="${1:?usage: build-incus-image.sh VERSION LINUX_ARCHIVE [OUT_DIR]}"
linux_archive="${2:?usage: build-incus-image.sh VERSION LINUX_ARCHIVE [OUT_DIR]}"
out_dir="${3:-target/incus-image-dist}"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
work_dir="$repo_root/target/incus-image-work"
image_definition="$repo_root/config/incus/labby-image.yaml"
image_name="labby-incus-x86_64-unknown-linux-gnu.tar.xz"
distrobuilder_bin="${DISTROBUILDER_BIN:-distrobuilder}"
secret_env_vars=(
    TS_AUTHKEY
    LABBY_MCP_HTTP_TOKEN
    OPENAI_API_KEY
    ANTHROPIC_API_KEY
    GITHUB_TOKEN
    GH_TOKEN
    NPM_TOKEN
    CARGO_REGISTRY_TOKEN
)
env_unset_args=()
for name in "${secret_env_vars[@]}"; do
    env_unset_args+=("-u" "$name")
done

if [[ "$distrobuilder_bin" == */* ]]; then
    [[ -x "$distrobuilder_bin" ]] || {
        printf 'distrobuilder binary is not executable: %s\n' "$distrobuilder_bin" >&2
        exit 1
    }
elif ! command -v "$distrobuilder_bin" >/dev/null 2>&1; then
    printf 'distrobuilder command not found: %s\n' "$distrobuilder_bin" >&2
    exit 1
fi

rm -rf "$work_dir" "$out_dir"
mkdir -p "$work_dir/files" "$work_dir/rootfs" "$out_dir"

tar -xzf "$linux_archive" -C "$work_dir/files" labby
test -x "$work_dir/files/labby"

python3 - "$image_definition" "$work_dir/labby-image.yaml" "$work_dir/files/labby" <<'PY'
from pathlib import Path
import sys

src, dst, binary = map(Path, sys.argv[1:])
text = src.read_text()
dst.write_text(text.replace("@@LABBY_BINARY@@", str(binary)))
PY

sudo env "${env_unset_args[@]}" "$distrobuilder_bin" build-incus \
    --type=unified \
    "$work_dir/labby-image.yaml" \
    "$work_dir/rootfs" \
    -o image.serial="$version"

built_image="$(find "$work_dir/rootfs" -maxdepth 1 -type f -name '*.tar.xz' -print -quit)"
test -n "$built_image"
install -D -m 0644 "$built_image" "$out_dir/$image_name"
(cd "$out_dir" && sha256sum "$image_name" > "$image_name.sha256")

printf '%s\n' "$out_dir/$image_name"
