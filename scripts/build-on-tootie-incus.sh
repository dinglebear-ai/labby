#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Build an exact Labby revision on TOOTIE inside the existing Labby Incus container.

Usage:
  scripts/build-on-tootie-incus.sh REVISION --base-image IMAGE [options]

Options:
  --base-image IMAGE   Qualified Labby runtime image used as the immutable base.
                       A digest-pinned reference or local image ID is required.
  --container NAME     Existing Incus container (default: labby-gateway).
  --host HOST          SSH target (default: tootie).
  --output PATH        Docker image archive output (default: target/tootie-build/...).
  --tag TAG            Tag applied to the exported image (default: labby:<short SHA>).
  --preflight-only     Verify the host/container/toolchain without copying source.
  --keep-workspace     Retain the isolated in-container source/build directory.
  -h, --help           Show this help.

The script never starts, stops, restarts, snapshots, or reconfigures the Incus
container. Source is copied into an isolated build directory and compiled as the
unprivileged labby user. The resulting binary is layered onto a caller-supplied,
already-qualified Labby runtime image on the TOOTIE Docker host, then exported
with `docker save` for transfer to another x86_64 Docker host.
EOF
}

die() {
    printf 'build-on-tootie-incus: %s\n' "$*" >&2
    exit 1
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
revision=""
ssh_host="tootie"
container="labby-gateway"
base_image=""
output=""
image_tag=""
keep_workspace=0
preflight_only=0

while (($#)); do
    case "$1" in
        --base-image)
            (($# >= 2)) || die "--base-image requires a value"
            base_image="$2"
            shift 2
            ;;
        --container)
            (($# >= 2)) || die "--container requires a value"
            container="$2"
            shift 2
            ;;
        --host)
            (($# >= 2)) || die "--host requires a value"
            ssh_host="$2"
            shift 2
            ;;
        --output)
            (($# >= 2)) || die "--output requires a value"
            output="$2"
            shift 2
            ;;
        --tag)
            (($# >= 2)) || die "--tag requires a value"
            image_tag="$2"
            shift 2
            ;;
        --keep-workspace)
            keep_workspace=1
            shift
            ;;
        --preflight-only)
            preflight_only=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --*)
            die "unknown option: $1"
            ;;
        *)
            [[ -z "$revision" ]] || die "only one revision may be supplied"
            revision="$1"
            shift
            ;;
    esac
done

[[ -n "$revision" ]] || die "REVISION is required"
[[ -n "$base_image" ]] || die "--base-image is required"
[[ "$base_image" == sha256:* || "$base_image" == *@sha256:* ]] \
    || die "--base-image must be a local image ID or digest-pinned reference"
[[ "$ssh_host" =~ ^[A-Za-z0-9._-]+$ ]] || die "invalid SSH host"
[[ "$container" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || die "invalid container name"

for command_name in git ssh scp sha256sum tar; do
    command -v "$command_name" >/dev/null 2>&1 || die "required command not found: $command_name"
done

commit="$(git -C "$repo_root" rev-parse --verify "${revision}^{commit}")" \
    || die "revision does not resolve to a commit: $revision"
short_commit="${commit:0:12}"
image_tag="${image_tag:-labby:${short_commit}}"
[[ "$image_tag" =~ ^[A-Za-z0-9._/:@-]+$ ]] || die "invalid image tag"

if [[ -z "$output" ]]; then
    output="$repo_root/target/tootie-build/labby-${short_commit}-linux-amd64.docker.tar"
elif [[ "$output" != /* ]]; then
    output="$PWD/$output"
fi
mkdir -p "$(dirname "$output")"

local_work="$(mktemp -d "${TMPDIR:-/tmp}/labby-tootie-build.XXXXXX")"
run_id="${commit}-$$"
remote_stage="/tmp/labby-build-${run_id}"
container_work="/home/labby/.cache/labby-builds/${run_id}"
source_archive="$local_work/source.tar"
binary_archive="$local_work/labby-linux-amd64.tar.gz"

cleanup() {
    rm -rf "$local_work"
    ssh -o BatchMode=yes "$ssh_host" \
        "rm -rf '$remote_stage'; if [ '$keep_workspace' -eq 0 ]; then incus_socket=\$(find /mnt/cache/incus/state /var/lib/incus -maxdepth 1 -type s -name unix.socket -print -quit 2>/dev/null); if [ -n \"\$incus_socket\" ]; then INCUS_DIR=\"\$(dirname \"\$incus_socket\")\" /usr/local/incus/bin/incus exec '$container' -- rm -rf '$container_work'; fi; fi" \
        >/dev/null 2>&1 || true
}
trap cleanup EXIT

printf '%s\n' "$commit" >"$local_work/REVISION"
git -C "$repo_root" archive --format=tar "$commit" >"$source_archive"
tar --append --file="$source_archive" -C "$local_work" REVISION
(cd "$local_work" && sha256sum source.tar >source.tar.sha256)

printf 'Preflighting %s container %s for revision %s\n' "$ssh_host" "$container" "$commit"
ssh -o BatchMode=yes "$ssh_host" sh -s -- "$container" <<'REMOTE_PREFLIGHT'
set -eu
container="$1"
incus_socket="$(find /mnt/cache/incus/state /var/lib/incus -maxdepth 1 -type s -name unix.socket -print -quit 2>/dev/null)"
[ -n "$incus_socket" ] || { echo "reachable Incus socket not found" >&2; exit 1; }
export INCUS_DIR="$(dirname "$incus_socket")"
incus=/usr/local/incus/bin/incus
[ -x "$incus" ] || { echo "Incus CLI not found" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq is required on the Incus host" >&2; exit 1; }
[ "$($incus list "$container" --format csv -c s)" = RUNNING ] \
    || { echo "Incus container is not running: $container" >&2; exit 1; }
[ "$($incus query "/1.0/instances/$container" | jq -r .architecture)" = x86_64 ] \
    || { echo "Incus container is not x86_64: $container" >&2; exit 1; }
$incus exec "$container" -- su -s /bin/bash - labby -c \
    'export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"; for tool in cargo node npx cc pkg-config readelf; do command -v "$tool" >/dev/null || { echo "missing build tool: $tool" >&2; exit 1; }; done'
command -v docker >/dev/null
docker info >/dev/null
REMOTE_PREFLIGHT

if [[ "$preflight_only" -eq 1 ]]; then
    printf 'Preflight passed for %s container %s (x86_64).\n' "$ssh_host" "$container"
    exit 0
fi

ssh -o BatchMode=yes "$ssh_host" "mkdir -p '$remote_stage'"
scp -q "$source_archive" "$source_archive.sha256" "$ssh_host:$remote_stage/"

printf 'Building revision %s inside Incus container %s\n' "$commit" "$container"
ssh -o BatchMode=yes "$ssh_host" sh -s -- \
    "$container" "$remote_stage" "$container_work" "$commit" <<'REBUILD'
set -euo pipefail
container="$1"; remote_stage="$2"; container_work="$3"; commit="$4"
incus_socket="$(find /mnt/cache/incus/state /var/lib/incus -maxdepth 1 -type s -name unix.socket -print -quit 2>/dev/null)"
export INCUS_DIR="$(dirname "$incus_socket")"
incus=/usr/local/incus/bin/incus
(cd "$remote_stage" && sha256sum --check --strict source.tar.sha256)
$incus exec "$container" -- rm -rf "$container_work"
$incus exec "$container" -- mkdir -p "$container_work"
$incus file push "$remote_stage/source.tar" "$container$container_work/source.tar"
$incus exec "$container" -- chown -R labby:labby "$container_work"
$incus exec "$container" -- su -s /bin/bash - labby -c "
set -euo pipefail
export PATH=\"\$HOME/.local/bin:\$HOME/.cargo/bin:\$PATH\"
cd '$container_work'
tar -xf source.tar
test \"\$(cat REVISION)\" = '$commit'
npx --yes pnpm@9.15.9 --dir apps/gateway-admin install --frozen-lockfile
npx --yes pnpm@9.15.9 --dir apps/gateway-admin build
CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \\
    cargo build --workspace --all-features --release --locked
test -x target/release/labby
readelf -h target/release/labby | grep -Eq 'Machine:[[:space:]]+Advanced Micro Devices X86-64'
tar -czf labby-linux-amd64.tar.gz -C target/release labby
sha256sum labby-linux-amd64.tar.gz > labby-linux-amd64.tar.gz.sha256
"
$incus file pull "$container$container_work/labby-linux-amd64.tar.gz" "$remote_stage/"
$incus file pull "$container$container_work/labby-linux-amd64.tar.gz.sha256" "$remote_stage/"
(cd "$remote_stage" && sha256sum --check --strict labby-linux-amd64.tar.gz.sha256)
REBUILD

scp -q "$ssh_host:$remote_stage/labby-linux-amd64.tar.gz" "$binary_archive"
scp -q "$ssh_host:$remote_stage/labby-linux-amd64.tar.gz.sha256" "$binary_archive.sha256"
(cd "$local_work" && sha256sum --check --strict "$(basename "$binary_archive").sha256")

printf 'Packaging Docker image %s on %s\n' "$image_tag" "$ssh_host"
ssh -o BatchMode=yes "$ssh_host" sh -s -- \
    "$remote_stage" "$base_image" "$image_tag" "$commit" <<'REPACKAGE'
set -euo pipefail
remote_stage="$1"; base_image="$2"; image_tag="$3"; commit="$4"
docker image inspect "$base_image" >/dev/null || docker pull "$base_image" >/dev/null
mkdir -p "$remote_stage/context"
tar -xzf "$remote_stage/labby-linux-amd64.tar.gz" -C "$remote_stage/context"
cat >"$remote_stage/context/Dockerfile" <<EOF
FROM $base_image
COPY --chmod=0755 labby /usr/local/bin/labby
LABEL ai.dinglebear.labby.source-revision=$commit
EOF
docker build --pull=false --tag "$image_tag" "$remote_stage/context" >/dev/null
docker image inspect "$image_tag" --format '{{.Id}}'
docker save --output "$remote_stage/labby.docker.tar" "$image_tag"
sha256sum "$remote_stage/labby.docker.tar" >"$remote_stage/labby.docker.tar.sha256"
REPACKAGE

scp -q "$ssh_host:$remote_stage/labby.docker.tar" "$output"
scp -q "$ssh_host:$remote_stage/labby.docker.tar.sha256" "$output.sha256"
expected="$(awk '{print $1}' "$output.sha256")"
actual="$(sha256sum "$output" | awk '{print $1}')"
[[ "$actual" = "$expected" ]] || die "exported Docker archive checksum mismatch"

printf 'Docker archive: %s\n' "$output"
printf 'SHA-256: %s\n' "$actual"
printf 'Source revision: %s\n' "$commit"
