#!/usr/bin/env bash
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
out=${1:?Usage: build-tailcat-bridge.sh ABSOLUTE_OUTPUT_DIRECTORY}
[[ "$out" == /* ]] || { echo 'Output directory must be absolute' >&2; exit 2; }
mkdir -p "$out"
cd "$repo/tools/tailcat-bridge"
go mod verify
go build -trimpath -o "$out/tailcat-bridge" .
GOOS=js GOARCH=wasm go build -trimpath -o "$out/tailcat.wasm" ./web
install -m 644 "$(go env GOROOT)/lib/wasm/wasm_exec.js" "$out/wasm_exec.js"
gzip -n -c "$out/tailcat.wasm" > "$out/tailcat.wasm.gz"
cp THIRD_PARTY_NOTICES.md "$out/THIRD_PARTY_NOTICES.md"
if command -v sha256sum >/dev/null; then
  (cd "$out" && sha256sum tailcat-bridge tailcat.wasm tailcat.wasm.gz wasm_exec.js THIRD_PARTY_NOTICES.md > SHA256SUMS)
else
  (cd "$out" && shasum -a 256 tailcat-bridge tailcat.wasm tailcat.wasm.gz wasm_exec.js THIRD_PARTY_NOTICES.md > SHA256SUMS)
fi
