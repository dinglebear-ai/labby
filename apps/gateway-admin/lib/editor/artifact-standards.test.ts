import assert from 'node:assert/strict'
import test from 'node:test'

import { composeArtifactSource, validateArtifactDraft } from './artifact-standards'

const metadata = {
  name: 'repo-triage',
  description: 'Triage a repository.',
  license: '',
  compatibility: '',
  allowedTools: 'Read Grep',
}

test('composes hidden metadata into standards-compatible skill frontmatter', () => {
  const source = composeArtifactSource('Skill', metadata, '# Workflow')
  assert.match(source, /^---\nname: "repo-triage"\ndescription: "Triage a repository\."/)
  assert.match(source, /allowed-tools: "Read Grep"\n---\n\n# Workflow$/)
})

test('validates Agent Skills naming and allowed-tools rules', () => {
  const issues = validateArtifactDraft('Skill', { ...metadata, name: 'Repo--Triage', allowedTools: '[Read, Grep]' }, '# Workflow')
  assert.equal(issues.filter((entry) => entry.severity === 'error').length, 2)
  assert.ok(issues.some((entry) => entry.field === 'name'))
  assert.ok(issues.some((entry) => entry.field === 'allowedTools'))
})

test('lints JSON-backed artifact bodies', () => {
  const issues = validateArtifactDraft('MCP', metadata, '{broken')
  assert.ok(issues.some((entry) => entry.field === 'content' && entry.severity === 'error'))
})
