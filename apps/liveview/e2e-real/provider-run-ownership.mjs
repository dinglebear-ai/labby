import {createHash, randomBytes} from 'node:crypto'
import {lstatSync, readFileSync, realpathSync, rmSync, writeFileSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {basename, dirname, isAbsolute, join, resolve} from 'node:path'
import {prepareProviderTls} from './provider-issuer.mjs'

const markerName = '.labby-provider-owner.json'

export function createOwnedProviderTls() {
  const tls = prepareProviderTls()
  const owner = {
    root: tls.root,
    spki: tls.spki,
    runId: randomBytes(32).toString('hex'),
    nonce: randomBytes(32).toString('hex'),
    pid: process.pid,
  }
  writeFileSync(join(owner.root, markerName), JSON.stringify(owner), {mode: 0o600, flag: 'wx'})
  return owner
}

export function cleanupOwnedProviderTls(owner) {
  if (!owner || owner.pid !== process.pid || !/^[a-f0-9]{64}$/.test(owner.runId || '') ||
      !/^[a-f0-9]{64}$/.test(owner.nonce || '') || typeof owner.root !== 'string') return false
  const root = owner.root
  if (!isAbsolute(root) || resolve(root) !== root || dirname(root) !== realpathSync(tmpdir()) ||
      !/^labby-controlled-idp-[A-Za-z0-9]{6}$/.test(basename(root))) return false
  try {
    if (lstatSync(root).isSymbolicLink() || !lstatSync(root).isDirectory() || realpathSync(root) !== root) {
      return false
    }
    const marker = join(root, markerName)
    const markerStat = lstatSync(marker)
    if (!markerStat.isFile() || markerStat.isSymbolicLink() || markerStat.size > 2048) return false
    const stored = JSON.parse(readFileSync(marker, 'utf8'))
    if (stored.root !== root || stored.pid !== owner.pid || stored.nonce !== owner.nonce ||
        stored.runId !== owner.runId || stored.spki !== owner.spki) return false
    rmSync(root, {recursive: true, force: true})
    return true
  } catch {
    return false
  }
}

export function validateProofSelection({binary, expectedSha256, sourceRevision}) {
  if (typeof binary !== 'string' || !isAbsolute(binary) || resolve(binary) !== binary ||
      !/^[a-f0-9]{64}$/.test(expectedSha256 || '') || !/^[a-f0-9]{40}$/.test(sourceRevision || '')) {
    throw new Error('PROVIDER_PROOF_SELECTION_INVALID')
  }
  const binarySha256 = createHash('sha256').update(readFileSync(binary)).digest('hex')
  if (binarySha256 !== expectedSha256) throw new Error('PROVIDER_PROOF_BINARY_MISMATCH')
  return {binary, binarySha256, sourceRevision}
}
