import test from 'node:test'
import assert from 'node:assert/strict'

import { artifactDescription, artifactExportFilename, artifactId, artifactKind, artifactLabel, collectArtifactKinds, filterArtifacts, serializeArtifact } from './library-model'

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
