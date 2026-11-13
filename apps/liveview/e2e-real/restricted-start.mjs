import {spawnSync} from 'node:child_process'
import {mkdirSync, writeFileSync, readFileSync, existsSync, rmSync} from 'node:fs'
import {resolve} from 'node:path'

export const restrictedPolicy = `config_version = 1
[[loadouts]]
name = "production"
upstreams = []
services = ["gateway"]
[[protected_mcp_routes]]
name = "operator"
enabled = true
public_host = "mcp.example.test"
public_path = "/operator"
scopes = ["lab:read"]
[protected_mcp_routes.target]
kind = "gateway_subset"
project_id = "bootstrap-default"
loadout = "production"
`

// The same public CLI/proof workflow exercised by Rust live_identity.rs.
// The generated product credential has real durable bindings and lab:read,
// not injected auth extensions or a second copy of an administrator cookie.
export async function startRestricted({binary, root, env, launch}) {
  const base = 'http://127.0.0.1:4301'
  for (const path of [root, resolve(root, 'home'), resolve(root, 'labby-home'), resolve(root, 'tmp'), resolve(root, 'logs')]) {
    mkdirSync(path, {recursive: true, mode: 0o700})
  }
  const privateEnv = {...env, HOME: resolve(root, 'home'), LABBY_HOME: resolve(root, 'labby-home'),
    TMPDIR: resolve(root, 'tmp'), LABBY_LOG_DIR: resolve(root, 'logs'),
    LABBY_AUTH_MODE: 'bearer', LABBY_PUBLIC_URL: 'https://lab.example.test',
    LABBY_WEB_UI_AUTH_DISABLED: 'false', LABBY_MCP_HTTP_TOKEN: process.env.LABBY_E2E_CREDENTIAL}
  const proofPath = resolve(root, 'proof.json')
  const credentialPath = resolve(root, 'credential.txt')
  writeFileSync(resolve(root, 'labby-home/config.toml'), restrictedPolicy, {mode: 0o600})
  const prepared = spawnSync(binary, ['auth', 'bootstrap', 'prepare', '--proof-file', proofPath,
    '--credential-file', credentialPath, '--organization-name', 'Browser E2E Org',
    '--project-name', 'Browser E2E Project', '--subject', 'restricted-e2e@example.test',
    '--loadout-id', 'production', '--route-id', 'operator', '--resource', 'https://mcp.example.test/operator',
    '--scope', 'lab:read', '--ttl', '600', '--json'],
  {env: privateEnv, encoding: 'utf8', timeout: 30000, maxBuffer: 65536})
  if (prepared.status !== 0) throw new Error('Restricted credential prepare failed (bounded CLI operation)')
  const {prepare_id: prepareId} = JSON.parse(prepared.stdout)
  const bundle = JSON.parse(readFileSync(proofPath, 'utf8'))
  let online = false
  const cleanup = async () => {
    if (!existsSync(proofPath)) return
    if (online) {
      const response = await fetch(`${base}/auth/bootstrap/cleanup`, {method: 'POST',
        headers: {'content-type': 'application/json', 'x-labby-bootstrap-proof': bundle.proof},
        body: JSON.stringify({prepare_id: prepareId}), signal: AbortSignal.timeout(10000)})
      if (response.status !== 200) throw new Error(`Restricted cleanup failed: HTTP ${response.status}`)
    } else {
      const result = spawnSync(binary, ['auth', 'bootstrap', 'recover', '--prepare-id', prepareId, '--revoke', '--json'],
        {env: privateEnv, timeout: 30000, maxBuffer: 65536, stdio: 'pipe'})
      if (result.status !== 0) throw new Error('Restricted offline recovery did not confirm revocation')
    }
    for (const path of [proofPath, credentialPath]) if (existsSync(path)) rmSync(path)
  }
  try {
    launch(binary, ['serve', '--host', '127.0.0.1', '--port', '4301', '--services', 'gateway,server_logs', '--no-input'],
      privateEnv, 'restricted-labby')
    let healthy = false
    for (let attempt = 0; attempt < 150; attempt++) {
      try { healthy = (await fetch(`${base}/health`, {signal: AbortSignal.timeout(1000)})).ok } catch {}
      if (healthy) break
      await new Promise(resolve => setTimeout(resolve, 200))
    }
    if (!healthy) throw new Error('Restricted daemon did not become healthy within deadline')
    online = true
    const consumed = await fetch(`${base}/auth/bootstrap/consume`, {method: 'POST',
      headers: {'content-type': 'application/json', 'x-labby-bootstrap-proof': bundle.proof},
      body: JSON.stringify(bundle.manifest), signal: AbortSignal.timeout(10000)})
    if (consumed.status !== 200) throw new Error(`Restricted consume failed: HTTP ${consumed.status}`)
    const credential = readFileSync(credentialPath, 'utf8').trim()
    const introspected = await fetch(`${base}/v1/access/credentials/self`, {
      headers: {authorization: `Bearer ${credential}`}, signal: AbortSignal.timeout(10000)})
    if (introspected.status !== 200) throw new Error(`Restricted introspection failed: HTTP ${introspected.status}`)
    const identity = await introspected.json()
    if (JSON.stringify(identity.scopes) !== '["lab:read"]') throw new Error('Restricted credential does not have expected scope ceiling')
    return {cleanup}
  } catch (error) {
    try { await cleanup() } catch { throw new Error(`${error.message}; restricted cleanup unconfirmed`) }
    throw error
  }
}
