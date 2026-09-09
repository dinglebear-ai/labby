import test from 'node:test'
import assert from 'node:assert/strict'
import { getDepotMembership } from './depot-membership-client'
import { __setBrowserSessionStateForTests } from '../auth/session-store'

const source = { connection_id: 'catalog', artifact_id: 'artifact', revision_id: 'revision' }
const response = (items: unknown[], library_version = 1) => new Response(JSON.stringify({ library_version, items }))

test('membership sends a bounded exact batch and accepts all defined statuses', async () => {
  const originalFetch = globalThis.fetch
  try {
    for (const status of ['exact_revision_present', 'different_revision_present', 'absent']) {
      globalThis.fetch = async (_url, init) => {
        assert.deepEqual(JSON.parse(String(init?.body)), { action: 'artifacts.depot_membership', params: { items: [source] } })
        return response([{ ...source, status }])
      }
      assert.equal((await getDepotMembership([source])).items[0].status, status)
    }
    await assert.rejects(getDepotMembership([]))
    await assert.rejects(getDepotMembership(Array.from({ length: 101 }, () => source)))
  } finally { globalThis.fetch = originalFetch }
})

test('membership rejects missing, mismatched, malformed and stale-authority answers', async () => {
  const originalFetch = globalThis.fetch
  try {
    for (const items of [[], [{ ...source, revision_id: 'other', status: 'exact_revision_present' }], [{ ...source, status: 'unknown' }], [{ ...source, status: 'absent', secret: true }]]) {
      globalThis.fetch = async () => response(items)
      await assert.rejects(getDepotMembership([source]))
    }
    globalThis.fetch = async () => {
      __setBrowserSessionStateForTests({ status: 'unauthenticated' })
      return response([{ ...source, status: 'exact_revision_present' }])
    }
    await assert.rejects(getDepotMembership([source]), /context changed/)
  } finally { globalThis.fetch = originalFetch }
})
