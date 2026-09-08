import test from 'node:test'
import assert from 'node:assert/strict'
import { revisionAge, selectDiscoveryResults } from './discover-model'
import type { FederatedArtifact } from '@/lib/api/depot-client'

const rows = [
  { providerId: 'team', artifactId: 'b', kind: 'skill', title: 'Beta' },
  { providerId: 'catalog', artifactId: 'a', descriptor: { kind: 'agent', title: 'Alpha' }, currentRevision: { authoredAt: '2026-09-08T00:00:00Z' } },
] as FederatedArtifact[]

test('kind filters include descriptor metadata and do not mutate catalog ordering', () => {
  assert.deepEqual(selectDiscoveryResults(rows, 'agent', 'catalog').map(row => row.artifactId), ['a'])
  assert.deepEqual(selectDiscoveryResults(rows, 'all', 'name').map(row => row.artifactId), ['a', 'b'])
  assert.deepEqual(rows.map(row => row.artifactId), ['b', 'a'])
})

test('newest ordering puts undated artifacts last without inventing dates', () => {
  assert.deepEqual(selectDiscoveryResults(rows, 'all', 'newest').map(row => row.artifactId), ['a', 'b'])
})

test('revision ages use real timestamps with deterministic server and future-date fallbacks', () => {
  const timestamp = '2026-09-08T10:00:00Z'
  const authored = Date.parse(timestamp)
  assert.equal(revisionAge(timestamp), '2026-09-08')
  assert.equal(revisionAge(timestamp, authored - 1), '2026-09-08')
  assert.equal(revisionAge(timestamp, Number.NaN), '2026-09-08')
  assert.equal(revisionAge('invalid', authored), '')
  for (const [seconds, expected] of [[0, 'just now'], [59, 'just now'], [60, '1m ago'], [3599, '59m ago'], [3600, '1h ago'], [86399, '23h ago'], [86400, '1d ago'], [604800, '1w ago'], [2592000, '2026-09-08']] as const) {
    assert.equal(revisionAge(timestamp, authored + seconds * 1000), expected)
  }
})
