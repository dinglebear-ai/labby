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
# A release companion directory carries the same signed, versioned manifest as
# the native helper. Source build output keeps the original SHA256SUMS contract.
if [[ -d "$build/tailcat" ]]; then build="$build/tailcat"; fi
modules="$root/packages/labby-tailcat-browser"
binaries="$build"
if [[ -f "$build/manifest.json" && -d "$build/browser" ]]; then
  node - "$build" <<'JS'
const fs=require('node:fs'),path=require('node:path'),crypto=require('node:crypto');
const root=process.argv[2],m=JSON.parse(fs.readFileSync(path.join(root,'manifest.json'),'utf8'));
if(m.schemaVersion!==1||m.protocol!==1||!Array.isArray(m.components))throw Error('unsupported companion manifest');
const expected=new Set();
for(const c of m.components){
 if(typeof c.path!=='string'||!c.path||c.path.split('/').some(p=>!p||p==='.'||p==='..')||c.path.includes('\\')||path.isAbsolute(c.path)||expected.has(c.path))throw Error('unsafe companion path');
 expected.add(c.path);const file=path.join(root,c.path);
 let cursor=root;for(const p of c.path.split('/')){cursor=path.join(cursor,p);if(fs.lstatSync(cursor).isSymbolicLink())throw Error('symlink companion');}
 if(!fs.statSync(file).isFile()||crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex')!==c.sha256)throw Error('companion checksum mismatch');
}
function walk(dir,prefix=''){for(const e of fs.readdirSync(dir,{withFileTypes:true})){const rel=prefix+e.name;if(e.isSymbolicLink())throw Error('symlink companion');if(e.isDirectory())walk(path.join(dir,e.name),rel+'/');else if(!e.isFile()||(rel!=='manifest.json'&&!expected.has(rel)))throw Error('unlisted companion');}}
walk(root);
JS
  modules="$build/browser"
  binaries="$build/browser"
else
  (cd "$build" && shasum -a 256 -c SHA256SUMS)
fi
target="$depot/priv/static/assets/tailcat"
mkdir -p "$target"
for file in tailcat.wasm wasm_exec.js; do install -m 644 "$binaries/$file" "$target/$file"; done
for file in http.mjs transport.mjs wasm.mjs hook.mjs; do install -m 644 "$modules/$file" "$target/$file"; done
node - "$target" "$map" <<'JS'
const fs=require('node:fs'),crypto=require('node:crypto'),path=require('node:path');
const root=process.argv[2];const digest=f=>crypto.createHash('sha256').update(fs.readFileSync(path.join(root,f))).digest('hex');
fs.writeFileSync(path.join(root,'manifest.json'),JSON.stringify({wasmSha256:digest('tailcat.wasm'),runtimeSha256:digest('wasm_exec.js'),derpMapURL:process.argv[3]}),{mode:0o644});
JS
# Notices produced by the pinned build accompany the binary assets.
for file in "$binaries"/*LICENSE* "$binaries"/*NOTICE*; do [[ ! -f $file ]] || install -m 644 "$file" "$target/$(basename "$file")"; done
