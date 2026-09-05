import assert from 'node:assert/strict'
import test from 'node:test'
import { depotCall, depotStatus, getArtifact, listArtifacts, listProviders, providerOperation, removeProvider, upsertProvider } from './depot-client.ts'

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

test('rejects incompatible contracts and unknown operations', async () => {
  await withFetch(json({ result: { artifacts: [] } }), async () => assert.rejects(depotCall('depot.artifacts.list', {}), /schemaVersion/i))
  await withFetch(json({ schemaVersion: 'labby.depot-compatibility/v2', result: { artifacts: [] } }), async () => assert.rejects(depotCall('depot.artifacts.list', {}), /schemaVersion/i))
  await withFetch(json({ result: { artifacts: [] } }), async () => assert.rejects(depotCall('depot.admin.execute', {}), /Unsupported Depot operation/))
})

test('preserves server error codes', async () => {
  await withFetch(json({ error: 'depot_unavailable' }, 502), async () => assert.rejects(depotStatus(), /depot_unavailable/))
})

const v2Page = { schemaVersion: 'labby.depot-compatibility/v2', scope: 'all', scopeEpoch: 'epoch', items: [{ providerId: 'public', artifactId: 'artifact-1', id: 'artifact-1' }], providerOutcomes: [{ providerId: 'public', state: 'exhausted' }], failures: [], coverageComplete: true, knownTotal: 1, totalIsExact: true, state: 'complete', nextCursor: null }

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
  globalThis.fetch = (async (url, init) => {
    requests.push({ url: String(url), init })
    return json({ operationId: 'op-1', version: 'v2', committed: true })
  }) as typeof fetch
  try {
    await upsertProvider({ id: 'team', name: 'Team', endpoint: 'https://depot.example', enabled: true, authMode: 'anonymous', credential: { action: 'retain' }, expectedVersion: 'v1', operationId: 'op-1' }, 'csrf-1')
    await removeProvider('team', 'v2', 'op-1', 'proof-1', 'csrf-2')
    await providerOperation('op-1')
  } finally { globalThis.fetch = original }
  assert.equal(new Headers(requests[0]?.init?.headers).get('x-csrf-token'), 'csrf-1')
  assert.equal(new Headers(requests[1]?.init?.headers).get('x-csrf-token'), 'csrf-2')
  assert.equal(requests[1]?.url, '/v1/depot/providers/team')
  assert.equal(requests[2]?.url, '/v1/depot/provider-operations/op-1')
})
