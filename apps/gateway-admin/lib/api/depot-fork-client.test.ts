import test from 'node:test'
import assert from 'node:assert/strict'
import { createDepotForkAttempt, prepareDepotFork } from './depot-fork-client'
import { __setBrowserSessionStateForTests } from '../auth/session-store'

const source = { connection_id: 'catalog', artifact_id: 'artifact', revision_id: 'revision' }
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status })
const admin = () => __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'admin' }, isAdmin: true, csrfToken: 'csrf', expiresAt: 9999999999 })

test('fork retains exact federated identity and submits once', async () => {
  const original = globalThis.fetch
  admin()
  let writes = 0
  globalThis.fetch = async (_url, init) => {
    const body = JSON.parse(String(init?.body))
    if (body.action === 'gateway.service_actions') return json([{ name: 'artifacts.fork', destructive: false, description: 'Fork' }])
    writes++
    assert.deepEqual(body, { action: 'artifacts.fork', params: { connection_id: 'catalog', source_artifact_id: 'artifact', revision_id: 'revision', namespace: 'team', name: 'copy', following: false } })
    return json({ artifact: { descriptor: { id: 'fork', namespace: 'team', name: 'copy' }, lineage: { forkedFromArtifactId: 'artifact', forkedFromRevisionId: 'revision' } } })
  }
  try {
    const ready = await prepareDepotFork(source)
    assert.equal(ready.status, 'ready')
    if (ready.status !== 'ready') return
    const attempt = createDepotForkAttempt(ready, 'team', 'copy')
    assert.equal(await attempt(), 'fork')
    await assert.rejects(attempt(), /already submitted/)
    assert.equal(writes, 1)
  } finally { globalThis.fetch = original }
})

test('fork fails closed on missing source, non-admin, missing action and changed epoch', async () => {
  const original = globalThis.fetch
  try {
    globalThis.fetch = async () => { throw new Error('unexpected request') }
    assert.equal((await prepareDepotFork()).status, 'blocked')
    admin()
    globalThis.fetch = async () => json([])
    assert.equal((await prepareDepotFork(source)).status, 'blocked')
    globalThis.fetch = async () => json([{ name: 'artifacts.fork', destructive: false }])
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    assert.equal((await prepareDepotFork(source)).status, 'blocked')
    admin()
    const ready = await prepareDepotFork(source)
    assert.equal(ready.status, 'ready')
    if (ready.status !== 'ready') return
    const attempt = createDepotForkAttempt(ready, 'team', 'copy')
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    await assert.rejects(attempt(), /session changed/)
  } finally { globalThis.fetch = original }
})

test('auth failure is structured and never retried automatically', async () => {
  const original = globalThis.fetch
  admin()
  let writes = 0
  globalThis.fetch = async (_url, init) => {
    const body = JSON.parse(String(init?.body))
    if (body.action === 'gateway.service_actions') return json([{ name: 'artifacts.fork', destructive: false }])
    writes++
    return json({ message: 'Permission revoked', kind: 'auth_failed', param: 'connection_id' }, 403)
  }
  try {
    const ready = await prepareDepotFork(source)
    if (ready.status !== 'ready') assert.fail('expected ready')
    await assert.rejects(createDepotForkAttempt(ready, 'team', 'copy')(), { message: 'Permission revoked', status: 403, code: 'auth_failed', param: 'connection_id' })
    assert.equal(writes, 1)
  } finally { globalThis.fetch = original }
})
