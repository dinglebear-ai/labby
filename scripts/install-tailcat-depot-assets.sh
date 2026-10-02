#!/usr/bin/env bash
# Install reproducible public browser assets into a disposable/deployment Depot tree.
set -euo pipefail
if [[ $# -ne 3 ]]; then
  echo 'usage: install-tailcat-depot-assets.sh BUILD_DIR DEPOT_DIR HTTPS_DERP_MAP_URL' >&2
  exit 2
fi
build=$1
depot=$2
map=$3
root=$(cd "$(dirname "$0")/.." && pwd)
[[ $build = /* && $depot = /* ]] || { echo 'absolute directories required' >&2; exit 2; }
node - "$map" <<'JS'
const u=new URL(process.argv[2]);if(u.protocol!=='https:'||u.username||u.password||u.hash)process.exit(2);
JS
(cd "$build" && shasum -a 256 -c SHA256SUMS)
target="$depot/priv/static/assets/tailcat"
mkdir -p "$target"
for file in tailcat.wasm wasm_exec.js; do install -m 644 "$build/$file" "$target/$file"; done
for file in http.mjs transport.mjs wasm.mjs hook.mjs; do install -m 644 "$root/packages/labby-tailcat-browser/$file" "$target/$file"; done
node - "$target" "$map" <<'JS'
const fs=require('node:fs'),crypto=require('node:crypto'),path=require('node:path');
const root=process.argv[2];const digest=f=>crypto.createHash('sha256').update(fs.readFileSync(path.join(root,f))).digest('hex');
fs.writeFileSync(path.join(root,'manifest.json'),JSON.stringify({wasmSha256:digest('tailcat.wasm'),runtimeSha256:digest('wasm_exec.js'),derpMapURL:process.argv[3]}),{mode:0o644});
JS
# Notices produced by the pinned build accompany the binary assets.
for file in "$build"/*LICENSE* "$build"/*NOTICE*; do [[ ! -f $file ]] || install -m 644 "$file" "$target/$(basename "$file")"; done
