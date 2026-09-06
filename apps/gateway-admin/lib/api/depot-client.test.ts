import assert from 'node:assert/strict'
import test from 'node:test'
import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { depotCall, depotOperations, depotStatus, getArtifact, listArtifacts, listProviders, providerOperation, removeProvider, upsertProvider } from './depot-client.ts'

async function withFetch(response: Response, run: () => Promise<void>) {
  const original = globalThis.fetch
  globalThis.fetch = (async () => response) as typeof fetch
  try { await run() } finally { globalThis.fetch = original }
}
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status })
const artifact = { id: 'artifact-1', kind: 'skill', name: 'demo' }

test('accepts complete status, list, and detail contracts', async () => {
  await withFetch(json({ depot: { configured: true, enabled: true, mutationAuthority: false, maxResponseBytes: 1_048_576 } }), async () => assert.equal((await depotStatus()).configured, true))
  await withFetch(json({ schemaVersion: 'labby.depot-compatibility/v1', result: { artifacts: [artifact], total: 1 } }), async () => assert.equal((await depotCall<{result:{artifacts:Array<{id:string}>}}>('depot.artifacts.list', {})).result.artifacts[0]?.id, 'artifact-1'))
  await withFetch(json({ schemaVersion: 'labby.depot-compatibility/v1', result: { artifact } }), async () => assert.equal((await depotCall<{result:{artifact:{id:string}}}>('depot.artifacts.get', {})).result.artifact.id, 'artifact-1'))
})

test('rejects invalid JSON even for successful HTTP responses', async () => {
  await withFetch(new Response('<html>', { status: 200 }), async () => assert.rejects(depotStatus(), /invalid JSON \(200\)/))
})

test('rejects missing status fields and wrong field types', async () => {
  await withFetch(json({ depot: { configured: true, enabled: 'yes', mutationAuthority: false } }), async () => assert.rejects(depotStatus(), /incompatible status response.*enabled/i))
})

test('requires result envelopes and typed pagination fields', async () => {
  await withFetch(json({ schemaVersion: 'labby.depot-compatibility/v1' }), async () => assert.rejects(depotCall('depot.artifacts.list', {}), /artifact list response.*result/i))
  await withFetch(json({ schemaVersion: 'labby.depot-compatibility/v1', result: { artifacts: [], total: '4' } }), async () => assert.rejects(depotCall('depot.artifacts.list', {}), /artifact list response.*total/i))
})

test('rejects artifacts without identity in list and detail results', async () => {
  await withFetch(json({ schemaVersion: 'labby.depot-compatibility/v1', result: { artifacts: [{ name: 'anonymous' }] } }), async () => assert.rejects(depotCall('depot.artifacts.list', {}), /artifact identity is missing/i))
  await withFetch(json({ schemaVersion: 'labby.depot-compatibility/v1', result: { artifact: { descriptor: { name: 'anonymous' } } } }), async () => assert.rejects(depotCall('depot.artifacts.get', {}), /artifact identity is missing/i))
})

test('accepts the canonical operation catalog and generic operation results', async () => {
  await withFetch(json({ operations: [{ name: 'depot.system.status', title: 'Depot status', description: 'Status', inputSchema: { type: 'object', properties: {}, required: [], additionalProperties: false } }] }), async () => assert.equal((await depotOperations())[0]?.name, 'depot.system.status'))
  await withFetch(json({ schemaVersion: 'labby.depot-compatibility/v1', result: { ok: true } }), async () => assert.equal((await depotCall<{ result: { ok: boolean } }>('depot.system.status', {})).result.ok, true))
})

test('rejects incompatible contracts and generic envelopes', async () => {
  await withFetch(json({ result: { artifacts: [] } }), async () => assert.rejects(depotCall('depot.artifacts.list', {}), /schemaVersion/i))
  await withFetch(json({ schemaVersion: 'labby.depot-compatibility/v2', result: { artifacts: [] } }), async () => assert.rejects(depotCall('depot.artifacts.list', {}), /schemaVersion/i))
  await withFetch(json({ result: { ok: true } }), async () => assert.rejects(depotCall('depot.admin.execute', {}), /schemaVersion/i))
})

test('preserves server error codes', async () => {
  await withFetch(json({ error: 'depot_unavailable' }, 502), async () => assert.rejects(depotStatus(), /depot_unavailable/))
})

const v2Page = { schemaVersion: 'labby.depot-compatibility/v2', scope: 'all', scopeEpoch: 'epoch', items: [{ providerId: 'public', artifactId: 'artifact-1', id: 'artifact-1' }], providerOutcomes: [{ providerId: 'public', state: 'exhausted' }], failures: [], coverageComplete: true, knownTotal: 1, totalIsExact: true, state: 'complete', nextCursor: null }

test('read-only v2 POST requests carry the authenticated browser CSRF token', async () => {
  const original = globalThis.fetch
  const csrfHeaders: Array<string | null> = []
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf-read', isAdmin: false })
  globalThis.fetch = (async (url, init) => {
    csrfHeaders.push(new Headers(init?.headers).get('x-csrf-token'))
    return String(url).endsWith('/discover')
      ? json(v2Page)
      : json({ schemaVersion: 'labby.depot-compatibility/v2', providerId: 'public', artifactId: 'artifact-1', artifact: { id: 'artifact-1' } })
  }) as typeof fetch
  try {
    await listArtifacts()
    await getArtifact('public', 'artifact-1')
  } finally {
    globalThis.fetch = original
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
  assert.deepEqual(csrfHeaders, ['csrf-read', 'csrf-read'])
})

test('read-only v2 POST refreshes a stale browser CSRF token once', async () => {
  const original = globalThis.fetch
  const csrfHeaders: Array<string | null> = []
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf-stale', isAdmin: false })
  globalThis.fetch = (async (url, init) => {
    if (String(url) === '/auth/session') {
      return json({ authenticated: true, user: { sub: 'operator' }, expires_at: Date.now() + 60_000, csrf_token: 'csrf-fresh', is_admin: false })
    }
    csrfHeaders.push(new Headers(init?.headers).get('x-csrf-token'))
    return csrfHeaders.length === 1
      ? json({ kind: 'validation_failed', message: 'invalid csrf token' }, 422)
      : json(v2Page)
  }) as typeof fetch
  try {
    assert.equal((await listArtifacts()).knownTotal, 1)
  } finally {
    globalThis.fetch = original
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
  assert.deepEqual(csrfHeaders, ['csrf-stale', 'csrf-fresh'])
})

test('read-only v2 POST bounds stale-session recovery to one retry', async () => {
  const original = globalThis.fetch
  const urls: string[] = []
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf-stale', isAdmin: false })
  globalThis.fetch = (async (url) => {
    urls.push(String(url))
    return String(url) === '/auth/session'
      ? json({ authenticated: true, user: { sub: 'operator' }, expires_at: Date.now() + 60_000, csrf_token: 'csrf-fresh', is_admin: false })
      : json({ kind: 'validation_failed', message: 'invalid csrf token' }, 422)
  }) as typeof fetch
  try {
    await assert.rejects(listArtifacts(), /invalid csrf token/)
  } finally {
    globalThis.fetch = original
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
  assert.deepEqual(urls, ['/v1/depot/discover', '/auth/session', '/v1/depot/discover'])
})

test('concurrent stale reads share one refresh and both retry with the fresh token', async () => {
  const original = globalThis.fetch
  const csrfHeaders: string[] = []
  let sessionRequests = 0
  let releaseSecondStaleResponse!: () => void
  const secondStaleResponse = new Promise<void>((resolve) => { releaseSecondStaleResponse = resolve })
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf-stale', isAdmin: false })
  globalThis.fetch = (async (url, init) => {
    if (String(url) === '/auth/session') {
      sessionRequests += 1
      releaseSecondStaleResponse()
      return json({ authenticated: true, user: { sub: 'operator' }, expires_at: Date.now() + 60_000, csrf_token: 'csrf-fresh', is_admin: false })
    }
    const csrf = new Headers(init?.headers).get('x-csrf-token') ?? ''
    csrfHeaders.push(csrf)
    if (csrf === 'csrf-fresh') return json(v2Page)
    if (csrfHeaders.filter(value => value === 'csrf-stale').length === 2) await secondStaleResponse
    return json({ kind: 'validation_failed', message: 'invalid csrf token' }, 422)
  }) as typeof fetch
  try {
    const [first, second] = await Promise.all([listArtifacts(), listArtifacts()])
    assert.equal(first.knownTotal, 1)
    assert.equal(second.knownTotal, 1)
  } finally {
    globalThis.fetch = original
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
  assert.equal(sessionRequests, 1)
  assert.deepEqual(csrfHeaders.sort(), ['csrf-fresh', 'csrf-fresh', 'csrf-stale', 'csrf-stale'].sort())
})

test('read-only v2 POST does not refresh for unrelated validation failures', async () => {
  const original = globalThis.fetch
  const urls: string[] = []
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf-current', isAdmin: false })
  globalThis.fetch = (async (url) => {
    urls.push(String(url))
    return json({ kind: 'validation_failed', message: 'invalid provider' }, 422)
  }) as typeof fetch
  try {
    await assert.rejects(listArtifacts(), /invalid provider/)
  } finally {
    globalThis.fetch = original
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
  assert.deepEqual(urls, ['/v1/depot/discover'])
})

test('v2 discovery rejects unknown fields, unsafe totals, and wrong scope', async () => {
  await withFetch(json({ ...v2Page, injected: true }), async () => assert.rejects(listArtifacts(), /unrecognized/i))
  await withFetch(json({ ...v2Page, knownTotal: Number.MAX_SAFE_INTEGER + 1 }), async () => assert.rejects(listArtifacts(), /9007199254740991/))
  await withFetch(json({ ...v2Page, scope: 'team' }), async () => assert.rejects(listArtifacts(), /wrong discovery scope/i))
})

test('v2 exact detail preserves raw IDs and verifies every identity field', async () => {
  const artifactId = 'space + % / 雪'
  const response = { schemaVersion: 'labby.depot-compatibility/v2', providerId: 'public', artifactId, artifact: { id: artifactId } }
  await withFetch(json(response), async () => assert.equal((await getArtifact('public', artifactId)).artifact.id, artifactId))
  await withFetch(json({ ...response, artifact: { id: 'other' } }), async () => assert.rejects(getArtifact('public', artifactId), /wrong artifact identity/i))
})

test('admin provider projection is strict and contains no credential material', async () => {
  const provider = { id: 'team', name: 'Team', endpoint: 'https://depot.example', enabled: true, authMode: 'bearer', builtin: false, configVersion: 'v1', credentialConfigured: true, health: { state: 'healthy', observedAt: null, provenance: null, retryNotBefore: null } }
  await withFetch(json([{ ...provider, token: 'secret' }]), async () => assert.rejects(listProviders(), /unrecognized/i))
  await withFetch(json([provider]), async () => assert.equal((await listProviders())[0]?.credentialConfigured, true))
})

test('provider mutations carry CSRF and preserve operation identity', async () => {
  const original = globalThis.fetch
  const requests: Array<{ url: string; init?: RequestInit }> = []
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'session-token', isAdmin: true })
  globalThis.fetch = (async (url, init) => {
    requests.push({ url: String(url), init })
    return json({ operationId: 'op-1', version: 'v2', committed: true })
  }) as typeof fetch
  try {
    await upsertProvider({ id: 'team', name: 'Team', endpoint: 'https://depot.example', enabled: true, authMode: 'anonymous', credential: { action: 'retain' }, expectedVersion: 'v1', operationId: 'op-1' }, 'csrf-1')
    await removeProvider('team', 'v2', 'op-1', 'proof-1', 'csrf-2')
    await providerOperation('op-1')
  } finally {
    globalThis.fetch = original
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
  assert.equal(new Headers(requests[0]?.init?.headers).get('x-csrf-token'), 'csrf-1')
  assert.equal(new Headers(requests[1]?.init?.headers).get('x-csrf-token'), 'csrf-2')
  assert.equal(requests[1]?.url, '/v1/depot/providers/team')
  assert.equal(requests[2]?.url, '/v1/depot/provider-operations/op-1')
})

test('provider mutations never refresh or replay after a CSRF rejection', async () => {
  const original = globalThis.fetch
  const urls: string[] = []
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'session-token', isAdmin: true })
  globalThis.fetch = (async (url) => {
    urls.push(String(url))
    return json({ kind: 'validation_failed', message: 'invalid csrf token' }, 422)
  }) as typeof fetch
  try {
    await assert.rejects(
      upsertProvider({ id: 'team', name: 'Team', endpoint: 'https://depot.example', enabled: true, authMode: 'anonymous', credential: { action: 'retain' }, expectedVersion: 'v1', operationId: 'op-1' }, 'explicit-token'),
      /invalid csrf token/,
    )
  } finally {
    globalThis.fetch = original
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
  assert.deepEqual(urls, ['/v1/depot/providers'])
})
