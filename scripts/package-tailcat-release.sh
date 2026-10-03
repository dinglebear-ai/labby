#!/usr/bin/env bash
# Stage the companions inside the same archive/provenance subject as Labby.
set -euo pipefail
[[ $# -ge 3 && $# -le 4 ]] || { echo 'usage: package-tailcat-release.sh ABSOLUTE_DIST VERSION TARGET [ABSOLUTE_BRIDGE_BUILD]' >&2; exit 2; }
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
node - "$repo" "$@" <<'JS'
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const {spawnSync} = require('node:child_process');
const [repo, dist, version, target, suppliedBuild] = process.argv.slice(2);
if (!path.isAbsolute(dist) || (suppliedBuild && !path.isAbsolute(suppliedBuild)) || !/^\d+\.\d+\.\d+$/.test(version) || !['x86_64-unknown-linux-gnu','aarch64-unknown-linux-gnu','aarch64-apple-darwin'].includes(target)) throw Error('invalid release arguments');
const run = (command,args,cwd,timeout) => {
  const result = spawnSync(command,args,{cwd,timeout,stdio:'inherit',env:{...process.env,npm_config_fetch_retries:'2',npm_config_fetch_timeout:'60000'}});
  if (result.error || result.status !== 0) throw Error(`companion command failed: ${command}`);
};
const digest = file => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
fs.mkdirSync(dist,{recursive:true});
const staging = fs.mkdtempSync(path.join(dist,'.tailcat-stage-'));
try {
  const build = suppliedBuild || path.join(staging,'build');
  if (!suppliedBuild) {
    const go = spawnSync('go',['version'],{encoding:'utf8',timeout:10000});
    if (go.status !== 0 || !/\bgo1\.27\.1\b/.test(go.stdout)) throw Error('Go 1.27.1 is required');
    run('bash',[path.join(repo,'scripts/build-tailcat-bridge.sh'),build],repo,900000);
  }
  const checksums = fs.readFileSync(path.join(build,'SHA256SUMS'),'utf8');
  for (const name of ['tailcat-bridge','tailcat.wasm','tailcat.wasm.gz','wasm_exec.js','THIRD_PARTY_NOTICES.md']) {
    const line = checksums.split('\n').find(line => line.endsWith(`  ${name}`));
    if (!line || line.slice(0,64) !== digest(path.join(build,name))) throw Error(`bridge checksum mismatch: ${name}`);
  }
  const root = path.join(staging,'tailcat');
  for (const folder of ['native','browser','adapter']) fs.mkdirSync(path.join(root,folder),{recursive:true});
  fs.copyFileSync(path.join(build,'tailcat-bridge'),path.join(root,'native/tailcat-bridge'));
  fs.chmodSync(path.join(root,'native/tailcat-bridge'),0o755);
  for (const name of ['tailcat.wasm','tailcat.wasm.gz','wasm_exec.js','THIRD_PARTY_NOTICES.md']) fs.copyFileSync(path.join(build,name),path.join(root,'browser',name));
  for (const name of ['hook.mjs','transport.mjs','http.mjs','wasm.mjs']) fs.copyFileSync(path.join(repo,'packages/labby-tailcat-browser',name),path.join(root,'browser',name));
  for (const name of ['server.mjs','cleanup.mjs','package.json','package-lock.json']) fs.copyFileSync(path.join(repo,'packages/labby-microsandbox',name),path.join(root,'adapter',name));
  const adapter = path.join(root,'adapter');
  if (JSON.parse(fs.readFileSync(path.join(adapter,'package.json'))).version !== version) throw Error('adapter release version mismatch');
  run('npm',['ci','--omit=dev','--ignore-scripts','--no-audit','--no-fund'],adapter,600000);
  // Executable links are unnecessary: this private adapter runs server.mjs directly.
  fs.rmSync(path.join(adapter,'node_modules/.bin'),{recursive:true,force:true});
  const components = [];
  const walk = relative => {
    for (const entry of fs.readdirSync(path.join(root,relative),{withFileTypes:true}).sort((a,b)=>a.name.localeCompare(b.name))) {
      const name = path.posix.join(relative,entry.name);
      if (entry.isDirectory()) walk(name);
      else if (entry.isFile()) components.push({path:name,sha256:digest(path.join(root,name))});
      else throw Error(`unsupported companion entry: ${name}`);
    }
  };
  walk('');
  fs.writeFileSync(path.join(root,'manifest.json'),JSON.stringify({schemaVersion:1,version,protocol:1,target,components},null,2)+'\n');
  const destination = path.join(dist,'tailcat');
  if (fs.existsSync(destination)) throw Error('refusing to replace an existing companion bundle');
  fs.renameSync(root,destination);
  console.log(`Staged Tailcat companions: ${components.length} verified components`);
} finally {
  fs.rmSync(staging,{recursive:true,force:true});
}
JS
