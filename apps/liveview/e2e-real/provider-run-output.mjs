import {randomBytes} from 'node:crypto'
import {lstatSync, mkdtempSync, readFileSync, realpathSync, readdirSync, rmSync, writeFileSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {basename, dirname, isAbsolute, join, resolve} from 'node:path'

const markerName = '.labby-provider-output-owner.json'
const keys = ['LABBY_E2E_PROVIDER_OUTPUT_ROOT', 'LABBY_E2E_PROVIDER_RUN_ID', 'LABBY_E2E_PROVIDER_OUTPUT_NONCE']

function readPrivateJson(path, maxBytes) {
  const stat = lstatSync(path)
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > maxBytes || (stat.mode & 0o077) !== 0) {
    throw Error('PROVIDER_RUN_OUTPUT_INVALID')
  }
  return JSON.parse(readFileSync(path, 'utf8'))
}

function currentOrAncestor(pid) {
  let candidate = process.pid
  for (let depth = 0; candidate > 1 && depth < 16; depth++) {
    if (candidate === pid) return true
    const status = readFileSync(`/proc/${candidate}/status`, 'utf8')
    candidate = Number(status.match(/^PPid:\s*(\d+)$/m)?.[1])
  }
  return false
}

export function validateProviderOutput(env = process.env) {
  try {
    const root = env[keys[0]]
    const runId = env[keys[1]]
    const nonce = env[keys[2]]
    if (typeof root !== 'string' || !isAbsolute(root) || resolve(root) !== root ||
        dirname(root) !== realpathSync(tmpdir()) || !/^labby-provider-output-[A-Za-z0-9]{6}$/.test(basename(root)) ||
        !/^[a-f0-9]{32}$/.test(runId || '') || !/^[a-f0-9]{64}$/.test(nonce || '')) throw Error()
    const stat = lstatSync(root)
    if (!stat.isDirectory() || stat.isSymbolicLink() || realpathSync(root) !== root ||
        (stat.mode & 0o077) !== 0) throw Error()
    const owner = readPrivateJson(join(root, markerName), 2048)
    if (owner.root !== root || owner.runId !== runId || owner.nonce !== nonce ||
        !Number.isSafeInteger(owner.pid) || !currentOrAncestor(owner.pid)) throw Error()
    return owner
  } catch {
    throw Error('PROVIDER_RUN_OUTPUT_INVALID')
  }
}

export function selectProviderOutput(env = process.env) {
  if (keys.some(key => env[key] !== undefined)) return validateProviderOutput(env)
  const owner = {root: mkdtempSync(join(realpathSync(tmpdir()), 'labby-provider-output-')),
    runId: randomBytes(16).toString('hex'), nonce: randomBytes(32).toString('hex'), pid: process.pid}
  writeFileSync(join(owner.root, markerName), JSON.stringify(owner), {mode: 0o600, flag: 'wx'})
  env[keys[0]] = owner.root
  env[keys[1]] = owner.runId
  env[keys[2]] = owner.nonce
  return owner
}

export function writeProviderAccessManifest(env, manifest) {
  const owner = validateProviderOutput(env)
  const body = JSON.stringify(manifest)
  if (manifest.runId !== owner.runId || Buffer.byteLength(body) > 16384) {
    throw Error('PROVIDER_ACCESS_MANIFEST_INVALID')
  }
  const path = join(owner.root, 'access-run.json')
  writeFileSync(path, body, {mode: 0o600, flag: 'wx'})
  return path
}

export function readProviderAccessManifest(env = process.env) {
  const owner = validateProviderOutput(env)
  const manifest = readPrivateJson(join(owner.root, 'access-run.json'), 16384)
  if (manifest.runId !== owner.runId) throw Error('PROVIDER_ACCESS_MANIFEST_INVALID')
  return manifest
}

export function cleanupOwnedProviderOutput(owner, env = process.env) {
  if (!owner || owner.pid !== process.pid) return false
  try {
    const selected = {[keys[0]]: owner.root, [keys[1]]: owner.runId, [keys[2]]: owner.nonce}
    validateProviderOutput(selected)
    if (readdirSync(owner.root).some(name => ![markerName, 'access-run.json'].includes(name))) return false
    rmSync(owner.root, {recursive: true})
    if (keys.every(key => env[key] === selected[key])) for (const key of keys) delete env[key]
    return true
  } catch {
    return false
  }
}
