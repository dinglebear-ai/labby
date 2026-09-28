#!/usr/bin/env bash
# Build the static UI before compiling a user-facing Labby binary.
set -euo pipefail

repo_root="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$repo_root/apps/gateway-admin"

if ! command -v pnpm >/dev/null 2>&1; then
    printf '%s\n' 'error: pnpm is required to build Labby from source; install the toolchain pinned in .mise.toml and apps/gateway-admin/package.json' >&2
    exit 1
fi

# A fresh checkout needs dependencies, including build-time devDependencies
# even when the caller exports NODE_ENV=production. Never rewrite the lockfile.
pnpm install --frozen-lockfile --prod=false
pnpm build

# Do not let Cargo embed an empty/partial export after a nominally successful
# frontend command. pnpm build also runs the app's bundle and build-ID checks.
if [[ ! -s out/index.html ]]; then
    printf '%s\n' 'error: web build did not produce a nonempty out/index.html; refusing to build a Labby binary without its UI' >&2
    exit 1
fi
if [[ ! -d out/_next/static ]] || [[ -z "$(find out/_next/static -type f -name '*.js' -size +0c -print -quit)" ]]; then
    printf '%s\n' 'error: web build did not produce JavaScript assets in out/_next/static; refusing to build a Labby binary without its UI' >&2
    exit 1
fi
