import test from 'node:test'
import assert from 'node:assert/strict'
import { selectDiscoveryResults } from './discover-model'
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
