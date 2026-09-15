import test from 'node:test'
import assert from 'node:assert/strict'
import { artifactMatchLabel, revisionAge, selectDiscoveryResults, selectDiscoveryShelf } from './discover-model'
import type { FederatedArtifact } from '@/lib/api/depot-client'

const rows = [
  { providerId: 'team', artifactId: 'b', kind: 'skill', title: 'Beta' },
  { providerId: 'catalog', artifactId: 'a', descriptor: { kind: 'agent', title: 'Alpha' }, currentRevision: { authoredAt: '2026-09-08T00:00:00Z' } },
] as FederatedArtifact[]

test('presentation ordering preserves every server-filtered row and does not mutate input', () => {
  assert.deepEqual(selectDiscoveryResults(rows, 'relevance').map(row => row.artifactId), ['b', 'a'])
  assert.deepEqual(selectDiscoveryResults(rows, 'name').map(row => row.artifactId), ['a', 'b'])
  assert.deepEqual(rows.map(row => row.artifactId), ['b', 'a'])
})

test('newest ordering puts undated artifacts last without inventing dates', () => {
  assert.deepEqual(selectDiscoveryResults(rows, 'newest').map(row => row.artifactId), ['a', 'b'])
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


test('reference shelves keep their own lens separate from display sorting', () => {
  const shelfRows = [
    { providerId: 'p', artifactId: 'hot', kind: 'mcp', metrics: { installs: 40, forks: 120, stars: 3 }, publisherVerified: true, currentRevision: { authoredAt: '2026-09-14T00:00:00Z' } },
    { providerId: 'p', artifactId: 'bundle', kind: 'loadout', metrics: { installs: 20, forks: 20, stars: 1 }, publisherVerified: false, currentRevision: { authoredAt: '2026-09-15T00:00:00Z' } },
    { providerId: 'p', artifactId: 'popular', kind: 'skill', metrics: { installs: 100, forks: 5, stars: 8 }, publisherVerified: true, currentRevision: { authoredAt: '2026-09-10T00:00:00Z' } },
  ] as FederatedArtifact[]
  assert.deepEqual(selectDiscoveryShelf(shelfRows, 'trending').map(row => row.artifactId), ['hot', 'bundle', 'popular'])
  assert.deepEqual(selectDiscoveryShelf(shelfRows, 'new').map(row => row.artifactId), ['bundle', 'hot', 'popular'])
  assert.deepEqual(selectDiscoveryShelf(shelfRows, 'popular').map(row => row.artifactId), ['popular', 'hot', 'bundle'])
  assert.equal(selectDiscoveryShelf(shelfRows, 'bundled')[0]?.artifactId, 'bundle')
  assert.deepEqual(selectDiscoveryShelf(shelfRows, 'forks').map(row => row.artifactId), ['hot'])
  assert.deepEqual(selectDiscoveryShelf(shelfRows, 'curated').map(row => row.artifactId), ['hot', 'popular'])
  assert.deepEqual(selectDiscoveryResults(selectDiscoveryShelf(shelfRows, 'trending'), 'stars').map(row => row.artifactId), ['popular', 'hot', 'bundle'])
})

test('search-match cues identify only fields actually present on a retained artifact', () => {
  const artifact = {
    providerId: 'catalog', artifactId: 'browser-pilot', kind: 'skill', name: 'Browser Pilot', namespace: 'team/tools',
    description: 'Automates browser QA', descriptor: { tags: ['playwright', 'qa'] },
    readme: { state: 'available', kind: 'readme', path: 'README.md', content: 'Supports screenshot review', revisionId: 'r1' },
  } as FederatedArtifact
  assert.equal(artifactMatchLabel(artifact, 'pilot'), 'matched in name')
  assert.equal(artifactMatchLabel(artifact, 'playwright'), 'matched in tags')
  assert.equal(artifactMatchLabel(artifact, 'team/tools'), 'matched in publisher')
  assert.equal(artifactMatchLabel(artifact, 'skill'), 'matched in kind')
  assert.equal(artifactMatchLabel(artifact, 'browser qa'), 'matched in description')
  assert.equal(artifactMatchLabel(artifact, 'screenshot'), 'matched in readme')
  assert.equal(artifactMatchLabel(artifact, 'missing'), undefined)
  assert.equal(artifactMatchLabel(artifact, '   '), undefined)
})

test('source metric rankings put missing values last and never manufacture verification', () => {
  const reported = [
    { providerId: 'source', artifactId: 'missing' },
    { providerId: 'source', artifactId: 'zero', metrics: { installs: 0, forks: 0 }, publisherVerified: false },
    { providerId: 'source', artifactId: 'known', metrics: { installs: 20, forks: 3 }, publisherVerified: true },
  ] as FederatedArtifact[]
  assert.deepEqual(selectDiscoveryResults(reported, 'installs').map(row => row.artifactId), ['known', 'zero', 'missing'])
  assert.deepEqual(selectDiscoveryResults(reported, 'forks').map(row => row.artifactId), ['known', 'zero', 'missing'])
  assert.deepEqual(selectDiscoveryResults(reported, 'verified').map(row => row.artifactId), ['known', 'missing', 'zero'])
  assert.deepEqual(reported.map(row => row.artifactId), ['missing', 'zero', 'known'])
})
