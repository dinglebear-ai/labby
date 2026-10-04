#!/usr/bin/env bash
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo/tools/tailcat-bridge"
go test -race ./...
# Go WASM has a small argv/environment buffer. The test runner needs no host secrets.
scratch=$(mktemp -d "${TMPDIR:-/tmp}/labby-tailcat-tests.XXXXXX")
wrapper="$scratch/wasm-exec"
cleanup() { rm -- "$wrapper"; rmdir -- "$scratch"; }
trap cleanup EXIT
node=$(node -p 'process.execPath')
runtime="$(go env GOROOT)/lib/wasm/wasm_exec_node.js"
printf '#!/usr/bin/env bash\nexec env -i %q %q "$@"\n' "$node" "$runtime" > "$wrapper"
chmod 700 "$wrapper"
GOOS=js GOARCH=wasm go test -exec "$wrapper" ./web
