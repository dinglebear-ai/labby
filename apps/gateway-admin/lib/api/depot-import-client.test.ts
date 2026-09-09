import test from 'node:test'
import assert from 'node:assert/strict'
import { createDepotImportAttempt } from './depot-import-client'
import { __setBrowserSessionStateForTests, getBrowserSessionEpoch, getBrowserSessionState } from '../auth/session-store'

test('exact import submits once and preserves source, revision, CAS, and request authority', async () => {
  const originalFetch = globalThis.fetch
  const originalSession = getBrowserSessionState()
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'importer' }, csrfToken: 'fixture-csrf', projectId: 'fixture-project', expiresAt: 9999999999 })
  let requests = 0
  const source = { connection_id: 'catalog', artifact_id: 'artifact', revision_id: 'revision' }
  globalThis.fetch = async (_url, init) => {
    requests++
    const body = JSON.parse(String(init?.body))
    assert.equal(body.action, 'artifacts.import')
    assert.deepEqual(body.params.source, { kind: 'depot', connection_id: 'catalog', artifact_id: 'artifact', revision_id: 'revision' })
    assert.equal(body.params.expected_library_version, 7)
    assert.match(body.params.idempotency_key, /^depot-import-/)
    assert.equal(new Headers(init?.headers).get('x-labby-project-id'), 'fixture-project')
    assert.equal(new Headers(init?.headers).get('x-csrf-token'), 'fixture-csrf')
    return Response.json({ outcome: 'committed', artifact_id: 'artifact', committed_library_version: 8, published_library_version: 8 })
  }
  try {
    const attempt = createDepotImportAttempt(source, 7, getBrowserSessionEpoch())
    source.artifact_id = 'changed-after-preparation'
    await attempt()
    await assert.rejects(attempt(), /already submitted/)
    assert.equal(requests, 1)
  } finally { globalThis.fetch = originalFetch; __setBrowserSessionStateForTests(originalSession) }
})

test('auth and uncertain transport failures never refresh authentication or replay an import', async () => {
  const originalFetch = globalThis.fetch
  try {
    for (const status of [401, 403, 0]) {
      let requests = 0
      globalThis.fetch = async () => {
        requests++
        if (!status) throw new TypeError('Network unavailable')
        return Response.json({ message: 'Permission revoked', kind: 'auth_failed', param: 'source' }, { status })
      }
      const attempt = createDepotImportAttempt({ connection_id: 'catalog', artifact_id: 'artifact', revision_id: 'revision' }, 1, getBrowserSessionEpoch())
      await assert.rejects(attempt(), status ? { message: 'Permission revoked', status, code: 'auth_failed', param: 'source' } : /Network unavailable/)
      await assert.rejects(attempt(), /already submitted/)
      assert.equal(requests, 1)
    }
  } finally { globalThis.fetch = originalFetch }
})

test('a session change before submission prevents the request', async () => {
  const originalFetch = globalThis.fetch
  const originalSession = getBrowserSessionState()
  globalThis.fetch = async () => { throw new Error('unexpected request') }
  try {
    const attempt = createDepotImportAttempt({ connection_id: 'catalog', artifact_id: 'artifact', revision_id: 'revision' }, 1, getBrowserSessionEpoch())
    __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'new-context' }, expiresAt: 9999999999, csrfToken: 'fixture-csrf' })
    await assert.rejects(attempt(), /session or project changed/)
  } finally { globalThis.fetch = originalFetch; __setBrowserSessionStateForTests(originalSession) }
})

test('import rejects a receipt for another artifact, transaction version, or unconfirmed outcome', async () => {
  const originalFetch = globalThis.fetch
  const receipt = { outcome: 'committed', artifact_id: 'artifact', committed_library_version: 2, published_library_version: 2 }
  try {
    for (const changed of [{ artifact_id: 'other' }, { committed_library_version: 3 }, { outcome: 'pending' }]) {
      globalThis.fetch = async () => Response.json({ ...receipt, ...changed })
      const attempt = createDepotImportAttempt({ connection_id: 'catalog', artifact_id: 'artifact', revision_id: 'revision' }, 1, getBrowserSessionEpoch())
      await assert.rejects(attempt())
    }
  } finally { globalThis.fetch = originalFetch }
})

test('a late import receipt cannot confirm success in a changed session', async () => {
  const originalFetch = globalThis.fetch
  const originalSession = getBrowserSessionState()
  let finish!: (response: Response) => void
  let requests = 0
  globalThis.fetch = async () => {
    requests++
    return new Promise<Response>(resolve => { finish = resolve })
  }
  try {
    const attempt = createDepotImportAttempt({ connection_id: 'catalog', artifact_id: 'artifact', revision_id: 'revision' }, 1, getBrowserSessionEpoch())
    const pending = attempt()
    __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'changed-during-import' }, expiresAt: 9999999999, csrfToken: 'fixture-csrf' })
    finish(Response.json({ outcome: 'committed', artifact_id: 'artifact', committed_library_version: 2, published_library_version: 2 }))
    await assert.rejects(pending, /session or project changed/)
    assert.equal(requests, 1)
  } finally { globalThis.fetch = originalFetch; __setBrowserSessionStateForTests(originalSession) }
})
