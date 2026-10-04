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
# A stopped cache publication must allow the exact verified release to retry.
# Fail after the real rename; no process is killed and all files are disposable.
mkdir -p "$fixture/publication-shim"
real_mv=$(command -v mv)
cat > "$fixture/publication-shim/mv" <<'SH'
#!/bin/sh
"$LABBY_TEST_REAL_MV" "$@" || exit $?
for destination do :; done
case "$destination" in
  "$LABBY_TEST_INTERRUPTED_INSTALL"/.labby-install/artifacts/*/tailcat) exit 77 ;;
esac
SH
chmod +x "$fixture/publication-shim/mv"
interrupted="$fixture/interrupted-bin"
one_digest=$(shasum -a 256 "$fixture/one/labby" | awk '{print $1}')
set +e
PATH="$fixture/publication-shim:$PATH" LABBY_TEST_REAL_MV="$real_mv" \
LABBY_TEST_INTERRUPTED_INSTALL="$interrupted" \
LABBY_INSTALL_DIR="$interrupted" LABBY_INSTALL_NO_SETUP=1 \
LABBY_INSTALL_LOCAL_BINARY="$fixture/one/labby" \
LABBY_INSTALL_LOCAL_SHA256="$one_digest" \
LABBY_INSTALL_LOCAL_COMPANIONS="$fixture/one/tailcat" \
sh "$root/scripts/install.sh" >"$fixture/interrupted.log" 2>&1
interrupted_status=$?
set -e
[[ "$interrupted_status" == 77 ]]
[[ ! -e "$interrupted/labby" ]]
if ! LABBY_INSTALL_DIR="$interrupted" LABBY_INSTALL_NO_SETUP=1 \
LABBY_INSTALL_LOCAL_BINARY="$fixture/one/labby" \
LABBY_INSTALL_LOCAL_SHA256="$one_digest" \
LABBY_INSTALL_LOCAL_COMPANIONS="$fixture/one/tailcat" \
sh "$root/scripts/install.sh" >"$fixture/retry.log" 2>&1; then
  cat "$fixture/retry.log" >&2
  echo 'same verified release could not retry interrupted cache publication' >&2
  exit 1
fi
[[ $("$interrupted/labby") == one ]]
grep -q one "$interrupted/tailcat/manifest.json"
# A checksum-command failure must not become an empty successful inventory hash.
mkdir -p "$fixture/hash-failure-shim" "$fixture/hash-failure/tailcat"
printf '#!/bin/sh\necho hash-failure\n' > "$fixture/hash-failure/labby"
printf '{"version":"hash-failure"}\n' > "$fixture/hash-failure/tailcat/manifest.json"
printf payload > "$fixture/hash-failure/tailcat/payload"
real_sha256sum=$(command -v sha256sum || true)
cat > "$fixture/hash-failure-shim/sha256sum" <<'SH'
#!/bin/sh
case "$1" in
  ./payload) exit 77 ;;
esac
if [ -n "$LABBY_TEST_REAL_SHA256SUM" ]; then
  exec "$LABBY_TEST_REAL_SHA256SUM" "$@"
fi
exec shasum -a 256 "$@"
SH
chmod +x "$fixture/hash-failure-shim/sha256sum"
if PATH="$fixture/hash-failure-shim:$PATH" LABBY_TEST_REAL_SHA256SUM="$real_sha256sum" \
LABBY_INSTALL_DIR="$fixture/hash-failure-bin" LABBY_INSTALL_NO_SETUP=1 \
LABBY_INSTALL_LOCAL_BINARY="$fixture/hash-failure/labby" \
LABBY_INSTALL_LOCAL_SHA256="$(shasum -a 256 "$fixture/hash-failure/labby" | awk '{print $1}')" \
LABBY_INSTALL_LOCAL_COMPANIONS="$fixture/hash-failure/tailcat" \
sh "$root/scripts/install.sh" >"$fixture/hash-failure.log" 2>&1; then
  echo 'checksum failure was accepted into verified companion cache' >&2
  exit 1
fi
[[ ! -e "$fixture/hash-failure-bin/labby" ]]
# Newline paths cannot be represented by the inventory's line-oriented format.
mkdir -p "$fixture/newline/tailcat"
printf '#!/bin/sh\necho newline\n' > "$fixture/newline/labby"
printf '{"version":"newline"}\n' > "$fixture/newline/tailcat/manifest.json"
printf original > "$fixture/newline/tailcat/"$'a\nb'
if LABBY_INSTALL_DIR="$fixture/newline-bin" LABBY_INSTALL_NO_SETUP=1 \
LABBY_INSTALL_LOCAL_BINARY="$fixture/newline/labby" \
LABBY_INSTALL_LOCAL_SHA256="$(shasum -a 256 "$fixture/newline/labby" | awk '{print $1}')" \
LABBY_INSTALL_LOCAL_COMPANIONS="$fixture/newline/tailcat" \
sh "$root/scripts/install.sh" >"$fixture/newline.log" 2>&1; then
  echo 'unrepresentable companion filename was accepted into verified cache' >&2
  exit 1
fi
[[ ! -e "$fixture/newline-bin/labby" ]]
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
