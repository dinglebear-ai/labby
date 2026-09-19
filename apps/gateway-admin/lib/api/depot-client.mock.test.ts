import assert from 'node:assert/strict'
import test from 'node:test'

process.env.NEXT_PUBLIC_MOCK_DATA = 'true'

test('mock Depot Library operations remain entirely local', async () => {
  const { depotCall } = await import('./depot-client.ts')
  const original = globalThis.fetch
  let networkRequests = 0
  globalThis.fetch = (async () => {
    networkRequests++
    throw new Error('mock Depot operations must not access the network')
  }) as typeof fetch
  try {
    const list = await depotCall<{ result: { artifacts: Array<{ id: string }> } }>('depot.artifacts.list', {})
    assert.ok(list.result.artifacts.length > 0)
    const artifactId = list.result.artifacts[0]!.id
    const detail = await depotCall<{ result: { artifact: { id: string } } }>('depot.artifacts.get', { artifactId })
    assert.equal(detail.result.artifact.id, artifactId)
    assert.equal(networkRequests, 0)
  } finally {
    globalThis.fetch = original
  }
})
