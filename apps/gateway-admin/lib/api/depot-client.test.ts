import assert from 'node:assert/strict'
import test from 'node:test'
import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { consumeOwnerLinkApproval, depotCall, depotOperations, depotStatus, depotPublishCapability, publishDepotSkill, getArtifact, listArtifacts, listProviders, providerOperation, removeProvider, upsertProvider } from './depot-client.ts'

async function withFetch(response: Response, run: () => Promise<void>) {
  const original = globalThis.fetch
  globalThis.fetch = (async () => response) as typeof fetch
  try { await run() } finally { globalThis.fetch = original }
}
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status })
const artifact = { id: 'artifact-1', kind: 'skill', name: 'demo' }

test('owner link confirmation sends only session CSRF and an empty body', async () => {
  const original = globalThis.fetch
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'google-user' }, expiresAt: Date.now() + 60000, csrfToken: 'csrf-link' })
  let requests = 0
  globalThis.fetch = (async (url, init) => {
    requests++
    assert.equal(url, '/v1/access/owner-link/consume')
    assert.equal(init?.method, 'POST')
    assert.equal(new Headers(init?.headers).get('x-csrf-token'), 'csrf-link')
    assert.deepEqual(JSON.parse(String(init?.body)), {})
    return json({ linked: true, projectId: 'existing-team' })
  }) as typeof fetch
  try {
    assert.deepEqual(await consumeOwnerLinkApproval(), { linked: true, projectId: 'existing-team' })
    assert.equal(requests, 1)
  } finally { globalThis.fetch = original; __setBrowserSessionStateForTests({ status: 'unauthenticated' }) }
})

test('owner linking requires a session and never retries a failed one-use approval', async () => {
  const original = globalThis.fetch
  let requests = 0
  globalThis.fetch = (async () => {
    requests++
    return json({ kind: 'forbidden', message: 'approval expired' }, 403)
  }) as typeof fetch
  try {
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    await assert.rejects(consumeOwnerLinkApproval(), /Sign in again/)
    assert.equal(requests, 0)
    __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'google-user' }, expiresAt: Date.now() + 60000, csrfToken: 'csrf-link' })
    await assert.rejects(consumeOwnerLinkApproval(), /approval expired/)
    assert.equal(requests, 1)
  } finally { globalThis.fetch = original; __setBrowserSessionStateForTests({ status: 'unauthenticated' }) }
})

test('publishing respects server capability and requires a valid receipt', async () => {
  await withFetch(json({ available: false, reason: 'project_session_required' }), async () => {
    assert.equal((await depotPublishCapability()).available, false)
  })
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'owner' }, expiresAt: Date.now() + 60000, csrfToken: 'csrf-test', projectId: 'test-project' })
  try {
    await withFetch(json({ status: 'accepted' }), async () => assert.rejects(publishDepotSkill('demo', 'source'), /incompatible publish receipt/))
  } finally { __setBrowserSessionStateForTests({ status: 'unauthenticated' }) }
})

test('publishing sends the complete source and CSRF once without retrying errors', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'owner' }, expiresAt: Date.now() + 60000, csrfToken: 'csrf-test', projectId: 'test-project' })
  const original = globalThis.fetch
  let requests = 0
  globalThis.fetch = (async (url, init) => {
    requests++
    assert.equal(url, '/v1/depot/publish')
    assert.equal(new Headers(init?.headers).get('x-csrf-token'), 'csrf-test')
    assert.deepEqual(JSON.parse(String(init?.body)), { name: 'demo', source: '---\nname: demo\n---\nComplete skill body' })
    return json({ kind: 'publish_failed', message: 'Check the submitted job before retrying.' }, 503)
  }) as typeof fetch
  try {
    await assert.rejects(publishDepotSkill('demo', '---\nname: demo\n---\nComplete skill body'), /Check the submitted job/)
    assert.equal(requests, 1)
  } finally { globalThis.fetch = original; __setBrowserSessionStateForTests({ status: 'unauthenticated' }) }
})

test('publishing discards a successful receipt after the browser identity changes', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'owner' }, expiresAt: Date.now() + 60000, csrfToken: 'csrf-test', projectId: 'test-project' })
  const original = globalThis.fetch
  let requests = 0
  globalThis.fetch = (async () => {
    requests++
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    return json({ jobId: 'accepted-before-logout', status: 'queued' })
  }) as typeof fetch
  try {
    await assert.rejects(publishDepotSkill('demo', 'source'), /Session changed/)
    assert.equal(requests, 1)
  } finally { globalThis.fetch = original; __setBrowserSessionStateForTests({ status: 'unauthenticated' }) }
})

test('accepts complete status, list, and detail contracts', async () => {
  await withFetch(json({ depot: { configured: true, enabled: true, authority: 'unknown', maxResponseBytes: 1_048_576 } }), async () => assert.equal((await depotStatus()).configured, true))
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

test('normalizes absent optional catalog metadata without accepting invalid identities or types', async () => {
  const published = { ...artifact, title: null, description: null, descriptor: { id: artifact.id, title: null }, currentRevision: { id: 'revision-1', createdAt: null }, lineage: { following: false, upstreamArtifactId: null } }
  await withFetch(json({ schemaVersion: 'labby.depot-compatibility/v1', result: { artifacts: [published], total: 1 } }), async () => {
    const response = await depotCall<{ result: { artifacts: Array<{ id: string; title?: string }> } }>('depot.artifacts.list', {})
    assert.equal(response.result.artifacts[0]?.id, artifact.id)
    assert.equal(response.result.artifacts[0]?.title, undefined)
  })
  await withFetch(json({ schemaVersion: 'labby.depot-compatibility/v1', result: { artifact: published } }), async () => {
    const response = await depotCall<{ result: { artifact: { currentRevision: { createdAt?: string } } } }>('depot.artifacts.get', {})
    assert.equal(response.result.artifact.currentRevision.createdAt, undefined)
  })
  for (const invalid of [{ id: null }, { ...artifact, title: 42 }]) {
    await withFetch(json({ schemaVersion: 'labby.depot-compatibility/v1', result: { artifacts: [invalid] } }), async () => assert.rejects(depotCall('depot.artifacts.list', {}), /incompatible artifact list response/))
  }
})

test('accepts the canonical operation catalog and generic operation results', async () => {
  await withFetch(json({ operations: [{ name: 'depot.system.status', title: 'Depot status', description: 'Status', inputSchema: { type: 'object', properties: {}, required: [], additionalProperties: false } }] }), async () => assert.equal((await depotOperations())[0]?.name, 'depot.system.status'))
  await withFetch(json({ schemaVersion: 'labby.depot-compatibility/v1', result: { ok: true } }), async () => assert.equal((await depotCall<{ result: { ok: boolean } }>('depot.system.status', {})).result.ok, true))
})

test('accepts bounded output schema metadata without weakening operation input validation', async () => {
  const operation = { name: 'depot.system.status', title: 'Depot status', description: 'Status', inputSchema: { type: 'object' } }
  const outputSchema = { type: 'object', properties: { result: { anyOf: [{ type: 'string' }, { type: 'null' }] } }, additionalProperties: false }
  await withFetch(json({ operations: [{ ...operation, outputSchema }] }), async () => assert.deepEqual((await depotOperations())[0]?.outputSchema, outputSchema))
  for (const invalid of [null, 'object', [], { description: 'x'.repeat(65_537) }]) {
    await withFetch(json({ operations: [{ ...operation, outputSchema: invalid }] }), async () => assert.rejects(depotOperations(), /operation catalog response/i))
  }
  await withFetch(json({ operations: [{ ...operation, outputSchema, unknownMetadata: true }] }), async () => assert.rejects(depotOperations(), /operation catalog response/i))
  await withFetch(json({ operations: [{ ...operation, outputSchema, inputSchema: { type: 'object', additionalProperties: true } }] }), async () => assert.rejects(depotOperations(), /additional properties are not supported/i))
})

test('sends destructive intent only when explicitly supplied', async () => {
  const original = globalThis.fetch
  const bodies: unknown[] = []
  globalThis.fetch = (async (_url, init) => {
    bodies.push(JSON.parse(String(init?.body)))
    return json({ schemaVersion: 'labby.depot-compatibility/v1', result: { ok: true } })
  }) as typeof fetch
  try {
    await depotCall('depot.system.status', {})
    await depotCall('depot.tokens.revoke', { tokenId: 'token-1' }, undefined, { confirmed: true, idempotencyKey: 'intent-1' })
  } finally { globalThis.fetch = original }
  assert.deepEqual(bodies, [
    { operation: 'depot.system.status', params: {} },
    { operation: 'depot.tokens.revoke', params: { tokenId: 'token-1' }, destructiveIntent: { confirmed: true, idempotencyKey: 'intent-1' } },
  ])
})

test('accepts the full operation catalog contract bound', async () => {
  const operation = { name: 'depot.system.status', title: 'Depot status', description: 'Status', inputSchema: { type: 'object' as const } }
  await withFetch(json({ operations: Array.from({ length: 1000 }, (_, index) => ({ ...operation, name: `depot.operation.${index}` })) }), async () => {
    assert.equal((await depotOperations()).length, 1000)
  })
  await withFetch(json({ operations: Array.from({ length: 1001 }, (_, index) => ({ ...operation, name: `depot.operation.${index}` })) }), async () => {
    await assert.rejects(depotOperations(), /operation catalog response/i)
  })
})

test('does not surface privileged Depot rejection details', async () => {
  await withFetch(json({ error: 'depot_rejected', status: 422, detail: JSON.stringify({ message: 'CAS audit requires a repair token', token: 'not surfaced' }) }, 502), async () => {
    await assert.rejects(depotCall('depot.maintenance.cas_audit', {}), (error: Error) => error.message === 'Depot request failed (502, depot_rejected)')
  })
  await withFetch(json({ error: 'depot_rejected', detail: { reason: 'migration is already running', secret: 'not surfaced' } }, 502), async () => {
    await assert.rejects(depotCall('depot.maintenance.migrate', {}), (error: Error) => !error.message.includes('migration'))
  })
})

test('rejects operation schemas outside the bounded renderer subset', async () => {
  const operation = (inputSchema: unknown) => json({ operations: [{ name: 'depot.test', title: 'Test', description: 'Test', inputSchema }] })
  await withFetch(operation({ type: 'object', properties: { only: { type: 'array', items: { type: 'string', description: 'Operation names to include' } } } }), async () => assert.equal((await depotOperations()).length, 1))
  await withFetch(operation({ type: 'object', properties: { only: { type: 'array', items: { type: 'string', description: 'x'.repeat(4097) } } } }), async () => assert.rejects(depotOperations(), /operation catalog response/i))
  await withFetch(operation({ type: 'object', properties: { bad: null } }), async () => assert.rejects(depotOperations(), /operation catalog response/i))
  await withFetch(operation({ type: 'object', properties: { bad: { type: 'null' } } }), async () => assert.rejects(depotOperations(), /operation catalog response/i))
  await withFetch(operation({ type: 'object', properties: { bad: { type: 'string', pattern: '[' } } }), async () => assert.rejects(depotOperations(), /valid regular expression/i))
  await withFetch(operation({ type: 'object', properties: Object.fromEntries(Array.from({ length: 129 }, (_, index) => [`p${index}`, { type: 'string' }])) }), async () => assert.rejects(depotOperations(), /128 properties/i))
  await withFetch(operation({ type: 'object', properties: {}, required: ['missing'] }), async () => assert.rejects(depotOperations(), /not declared/i))
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

test('federated details accept the declared license returned by public discovery', async () => {
  for (const declared of ['MIT', null]) {
    const response = { schemaVersion: 'labby.depot-compatibility/v2', providerId: 'public', artifactId: 'artifact-1', artifact: { id: 'artifact-1', license: { declared, redistribution: 'allowed', reviewState: 'unreviewed' } } }
    await withFetch(json(response), async () => {
      assert.equal((await getArtifact('public', 'artifact-1')).artifact.license?.declared, declared)
    })
  }
})

test('federated licenses reject structured or oversized declarations and unknown fields', async () => {
  for (const license of [{ declared: { text: 'MIT' } }, { declared: 'x'.repeat(1025) }, { declared: 'MIT', privateEvidence: 'not public' }]) {
    await withFetch(json({ ...v2Page, items: [{ ...v2Page.items[0], license }] }), async () => {
      await assert.rejects(listArtifacts(), /incompatible discovery response/)
    })
  }
})

test('federated timestamp projection preserves authoredAt and nullable artifact dates', async () => {
  const item = { ...v2Page.items[0], createdAt: null, updatedAt: '2026-09-08T00:00:00Z', currentRevision: { id: 'revision-1', authoredAt: '2026-09-07T00:00:00Z' } }
  await withFetch(json({ ...v2Page, items: [item] }), async () => {
    const result = await listArtifacts()
    assert.equal(result.items[0]?.createdAt, null)
    assert.equal(result.items[0]?.currentRevision?.authoredAt, '2026-09-07T00:00:00Z')
  })
})

test('federated timestamps reject structured payloads and undocumented revision dates', async () => {
  for (const extra of [{ createdAt: { secret: 'not metadata' } }, { currentRevision: { createdAt: '2026-09-08T00:00:00Z' } }]) {
    await withFetch(json({ ...v2Page, items: [{ ...v2Page.items[0], ...extra }] }), async () => {
      await assert.rejects(listArtifacts(), /incompatible discovery response/)
    })
  }
})

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

test('v2 list and detail preserve only bounded integer file counts', async () => {
  for (const fileCount of [undefined, 0, 1, 2000, null, -1, 1.5, '3', {}, [], 2001]) {
    const currentRevision = fileCount === undefined ? {} : { fileCount }
    const valid = fileCount === undefined || (typeof fileCount === 'number' && Number.isInteger(fileCount) && fileCount >= 0 && fileCount <= 2000)
    await withFetch(json({ ...v2Page, items: [{ ...v2Page.items[0], currentRevision }] }), async () => {
      if (valid) assert.equal((await listArtifacts()).items[0]?.currentRevision?.fileCount, fileCount)
      else await assert.rejects(listArtifacts())
    })
    await withFetch(json({ schemaVersion: 'labby.depot-compatibility/v2', providerId: 'public', artifactId: 'artifact-1', artifact: { id: 'artifact-1', currentRevision } }), async () => {
      if (valid) assert.equal((await getArtifact('public', 'artifact-1')).artifact.currentRevision?.fileCount, fileCount)
      else await assert.rejects(getArtifact('public', 'artifact-1'))
    })
  }
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
  await withFetch(json([{ ...provider, endpoint: 'http://127.0.0.1:4100/', hostManaged: true }]), async () => assert.equal((await listProviders())[0]?.hostManaged, true))
  await withFetch(json([{ ...provider, hostManaged: 'true' }]), async () => assert.rejects(listProviders(), /boolean/i))
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
