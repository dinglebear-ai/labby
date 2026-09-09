import test from 'node:test'
import assert from 'node:assert/strict'

import { artifactDescription, artifactExportFilename, artifactId, artifactKind, artifactLabel, collectArtifactKinds, collectArtifactTags, filterArtifacts, filterLibraryArtifacts, sortLibraryArtifacts, serializeArtifact } from './library-model'

const artifacts = [
  { id: 'art_agent', kind: 'agent', namespace: 'acp', name: 'helper', title: 'Helper', description: 'Helps' },
  { descriptor: { id: 'art_skill', kind: 'skill', namespace: 'skills', name: 'review', title: 'Review', description: 'Reviews' } },
]

test('library model normalizes top-level and descriptor artifact fields', () => {
  assert.equal(artifactId(artifacts[1]), 'art_skill')
  assert.equal(artifactKind(artifacts[1]), 'skill')
  assert.equal(artifactLabel(artifacts[1]), 'Review')
  assert.equal(artifactDescription(artifacts[1]), 'Reviews')
  assert.deepEqual(collectArtifactKinds(artifacts), ['agent', 'skill'])
})

test('library kind filter uses normalized artifact kind', () => {
  assert.deepEqual(filterArtifacts(artifacts, 'skill'), [artifacts[1]])
  assert.equal(filterArtifacts(artifacts, 'all').length, 2)
})

test('library normalizes plural and protocol-specific kinds for stable filters', () => {
  assert.equal(artifactKind({ kind: 'MCP Server' }), 'mcp')
  assert.equal(artifactKind({ kind: 'ACP Agent' }), 'acp')
  assert.equal(artifactKind({ kind: 'Marketplaces' }), 'marketplace')
  assert.equal(artifactKind({ descriptor: { kind: 'Prompts' } }), 'prompt')
})

test('library exports portable, readable artifact metadata', () => {
  assert.equal(artifactExportFilename({ name: 'Review / Triage!' }), 'review-triage.depot.json')
  assert.deepEqual(JSON.parse(serializeArtifact(artifacts[0])), artifacts[0])
  assert.match(serializeArtifact(artifacts[0]), /\n$/)
})

test('tag facets count each tagged artifact once and preserve literal tag identity', () => {
  const rows = [
    { id: 'one', descriptor: { tags: ['rust', 'rust', '__proto__'] } },
    { id: 'two', descriptor: { tags: ['rust', 'Rust'] } },
    { id: 'three' },
  ]
  const counts = new Map(collectArtifactTags(rows).map(({ tag, count }) => [tag, count]))
  assert.equal(counts.get('rust'), 2)
  assert.equal(counts.get('Rust'), 1)
  assert.equal(counts.get('__proto__'), 1)
  assert.equal(counts.size, 3)
  assert.deepEqual(collectArtifactTags([]), [])
})

test('tag and kind filters intersect without admitting artifacts missing tags', () => {
  const rows = [
    { id: 'one', kind: 'skill', descriptor: { tags: ['rust'] } },
    { id: 'two', kind: 'agent', descriptor: { tags: ['rust'] } },
    { id: 'three', kind: 'skill' },
  ]
  assert.deepEqual(filterLibraryArtifacts(rows, 'skill', 'rust'), [rows[0]])
  assert.deepEqual(filterLibraryArtifacts(rows, 'all', 'rust'), rows.slice(0, 2))
  assert.deepEqual(filterLibraryArtifacts(rows, 'all', 'Rust'), [])
  assert.deepEqual(filterLibraryArtifacts(rows, 'skill', null), [rows[0], rows[2]])
})

test('name and kind sort use displayed values without mutating catalog order', () => {
  const rows = [
    { id: 'a', title: 'Zulu', kind: 'agent' },
    { id: 'b', descriptor: { title: 'Alpha', kind: 'skill' } },
    { id: 'c', title: 'Beta', kind: 'agent' },
  ]
  const original = structuredClone(rows)
  assert.deepEqual(sortLibraryArtifacts(rows, 'name').map(artifactId), ['b', 'c', 'a'])
  assert.deepEqual(sortLibraryArtifacts(rows, 'kind').map(artifactId), ['c', 'a', 'b'])
  assert.deepEqual(sortLibraryArtifacts(rows, 'catalog'), original)
  assert.notEqual(sortLibraryArtifacts(rows, 'catalog'), rows)
  assert.deepEqual(rows, original)
})
