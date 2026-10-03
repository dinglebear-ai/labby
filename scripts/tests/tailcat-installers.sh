#!/usr/bin/env bash
# Disposable installer activation, rollback, and extracted browser asset fixtures.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
for version in one two; do
  mkdir -p "$fixture/$version/tailcat/browser"
  printf '#!/bin/sh\necho %s\n' "$version" > "$fixture/$version/labby"
  printf '{"version":"%s"}\n' "$version" > "$fixture/$version/tailcat/manifest.json"
done
activate() {
  LABBY_INSTALL_DIR="$fixture/bin" LABBY_INSTALL_NO_SETUP=1 \
  LABBY_INSTALL_LOCAL_BINARY="$fixture/$1/labby" \
  LABBY_INSTALL_LOCAL_SHA256="$(shasum -a 256 "$fixture/$1/labby" | awk '{print $1}')" \
  LABBY_INSTALL_LOCAL_COMPANIONS="$fixture/$1/tailcat" \
  sh "$root/scripts/install.sh" >"$fixture/install.log" 2>&1
}
activate one
activate two
[[ $("$fixture/bin/labby") == two ]]
grep -q two "$fixture/bin/tailcat/manifest.json"
LABBY_INSTALL_DIR="$fixture/bin" LABBY_INSTALL_ROLLBACK=1 sh "$root/scripts/install.sh" >>"$fixture/install.log" 2>&1
[[ $("$fixture/bin/labby") == one ]]
grep -q one "$fixture/bin/tailcat/manifest.json"
# An interrupted pair activation restores both old components on recovery.
journal="$fixture/bin/.labby-install/activation-journal"
mkdir "$journal"
cp "$fixture/bin/labby" "$journal/old-binary"
touch "$journal/old-binary.present" "$journal/companions-managed" "$journal/old-tailcat.present"
cp -R "$fixture/bin/tailcat" "$journal/old-tailcat"
echo prepared > "$journal/state"
cp "$fixture/two/labby" "$fixture/bin/labby"
cp "$fixture/two/tailcat/manifest.json" "$fixture/bin/tailcat/manifest.json"
LABBY_INSTALL_DIR="$fixture/bin" LABBY_INSTALL_RECOVER_ONLY=1 sh "$root/scripts/install.sh" >>"$fixture/install.log" 2>&1
[[ $("$fixture/bin/labby") == one ]]
grep -q one "$fixture/bin/tailcat/manifest.json"
# Rollback refuses altered cached companions even when the manifest is intact.
# The synthetic recovery above intentionally omitted receipt backups.
activate one
activate two
one_digest=$(shasum -a 256 "$fixture/one/labby" | awk '{print $1}')
cached="$fixture/bin/.labby-install/artifacts/$one_digest/tailcat"
echo injected > "$cached/unlisted-file"
if LABBY_INSTALL_DIR="$fixture/bin" LABBY_INSTALL_ROLLBACK=1 sh "$root/scripts/install.sh" >"$fixture/corrupt-cache.log" 2>&1; then
  echo 'corrupted companion cache accepted' >&2; exit 1
fi
[[ $("$fixture/bin/labby") == two ]]
rm "$cached/unlisted-file"
LABBY_INSTALL_DIR="$fixture/bin" LABBY_INSTALL_ROLLBACK=1 sh "$root/scripts/install.sh" >>"$fixture/install.log" 2>&1
[[ $("$fixture/bin/labby") == one ]]
# Asset installation works with an extracted release and no checkout-relative JS.
node - "$fixture/release/tailcat" <<'JS'
const fs=require('node:fs'),path=require('node:path'),crypto=require('node:crypto');
const root=process.argv[2];fs.mkdirSync(path.join(root,'browser'),{recursive:true});
const files=['hook.mjs','http.mjs','transport.mjs','wasm.mjs','tailcat.wasm','wasm_exec.js'];
const components=files.map(name=>{const rel='browser/'+name,data=Buffer.from(name);fs.writeFileSync(path.join(root,rel),data);return {path:rel,sha256:crypto.createHash('sha256').update(data).digest('hex')};});
fs.writeFileSync(path.join(root,'manifest.json'),JSON.stringify({schemaVersion:1,protocol:1,version:'1.0.0',components}));
JS
bash "$root/scripts/install-tailcat-depot-assets.sh" "$fixture/release" "$fixture/depot" https://relay.example/map
cmp "$fixture/release/tailcat/browser/hook.mjs" "$fixture/depot/priv/static/assets/tailcat/hook.mjs"
echo tampered > "$fixture/release/tailcat/browser/hook.mjs"
if bash "$root/scripts/install-tailcat-depot-assets.sh" "$fixture/release" "$fixture/depot" https://relay.example/map >"$fixture/invalid.log" 2>&1; then
  echo 'tampered companion accepted' >&2; exit 1
fi
echo 'Tailcat installer activation, recovery, rollback, and extracted assets passed'
