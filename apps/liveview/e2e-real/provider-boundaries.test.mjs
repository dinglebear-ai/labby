import {test} from 'node:test'
import {createHash} from 'node:crypto'
import assert from 'node:assert/strict'
import {existsSync, mkdtempSync, mkdirSync, rmSync, symlinkSync, writeFileSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {requireWrongIssuerRejection} from './provider-provision.mjs'
import {createOwnedProviderTls, cleanupOwnedProviderTls, validateProofSelection} from './provider-run-ownership.mjs'

test('only the actual wrong-issuer authentication rejection qualifies', () => {
  assert.doesNotThrow(() => requireWrongIssuerRejection({status: 401}))
  for (const status of [200, 400, 403, 500, 503, undefined]) {
    assert.throws(() => requireWrongIssuerRejection({status}), /PROVIDER_WRONG_ISSUER_EXPECTED_401/)
  }
  assert.throws(() => requireWrongIssuerRejection(undefined), /PROVIDER_WRONG_ISSUER_EXPECTED_401/)
})

test('owned TLS cleanup requires exact current-run path and matching marker', () => {
  const owner = createOwnedProviderTls()
  const foreign = mkdtempSync(join(tmpdir(), 'labby-controlled-idp-'))
  const sibling = `${owner.root}-sibling`
  const link = mkdtempSync(join(tmpdir(), 'labby-controlled-idp-'))
  rmSync(link, {recursive: true})
  mkdirSync(sibling)
  symlinkSync(foreign, link)
  try {
    for (const altered of [
      {...owner, root: foreign},
      {...owner, root: `${owner.root}/../${foreign.split('/').at(-1)}`},
      {...owner, root: sibling},
      {...owner, root: link},
      {...owner, nonce: 'foreign'},
      {...owner, runId: 'foreign'},
      {...owner, pid: process.pid + 1},
    ]) {
      assert.equal(cleanupOwnedProviderTls(altered), false)
      assert.ok(existsSync(owner.root))
      assert.ok(existsSync(foreign))
    }
    writeFileSync(join(owner.root, '.labby-provider-owner.json'), JSON.stringify({...owner, nonce: 'foreign'}))
    assert.equal(cleanupOwnedProviderTls(owner), false)
    writeFileSync(join(owner.root, '.labby-provider-owner.json'), JSON.stringify(owner))
    assert.equal(cleanupOwnedProviderTls(owner), true)
    assert.equal(existsSync(owner.root), false)
  } finally {
    for (const path of [owner.root, foreign, sibling, link]) rmSync(path, {recursive: true, force: true})
  }
})

test('externally supplied TLS survives inherited ownership flags and teardown', async () => {
  const tls = createOwnedProviderTls()
  const keys = ['LABBY_E2E_PROVIDER_TLS', 'LABBY_E2E_PROVIDER_SPKI', 'LABBY_E2E_PROVIDER_TLS_OWNED']
  const previous = keys.map(key => process.env[key])
  try {
    process.env.LABBY_E2E_PROVIDER_TLS = tls.root
    process.env.LABBY_E2E_PROVIDER_SPKI = tls.spki
    process.env.LABBY_E2E_PROVIDER_TLS_OWNED = 'true'
    const {default: config} = await import(`../playwright.provider.config.mjs?foreign=${Date.now()}`)
    const {default: teardown} = await import('./provider-teardown.mjs')
    assert.equal(config.metadata?.providerTlsOwner, undefined)
    await teardown(config)
    assert.ok(existsSync(tls.root))
  } finally {
    keys.forEach((key, index) => {
      if (previous[index] === undefined) delete process.env[key]
      else process.env[key] = previous[index]
    })
    cleanupOwnedProviderTls(tls)
  }
})

test('proof selection requires an absolute readable binary and exact SHA/full revision', () => {
  const root = mkdtempSync(join(tmpdir(), 'labby-provider-selection-'))
  const binary = join(root, 'selected')
  writeFileSync(binary, 'known binary bytes')
  const selection = {binary, expectedSha256: 'b'.repeat(64), sourceRevision: 'a'.repeat(40)}
  try {
    assert.throws(() => validateProofSelection(selection), /PROVIDER_PROOF_BINARY_MISMATCH/)
    const expectedSha256 = createHash('sha256').update('known binary bytes').digest('hex')
    assert.deepEqual(validateProofSelection({...selection, expectedSha256}), {
      binary, binarySha256: expectedSha256, sourceRevision: selection.sourceRevision,
    })
    for (const invalid of [
      {...selection, expectedSha256: undefined},
      {...selection, sourceRevision: 'short'},
      {...selection, binary: 'relative-binary'},
    ]) assert.throws(() => validateProofSelection(invalid), /PROVIDER_PROOF_SELECTION_INVALID/)
  } finally {
    rmSync(root, {recursive: true, force: true})
  }
})
