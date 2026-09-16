import assert from 'node:assert/strict'
import test from 'node:test'

import { artifactFrontmatterFields, composeArtifactSource, validateArtifactDraft } from './artifact-standards'

const metadata = {
  name: 'repo-triage',
  description: 'Triage a repository.',
  tags: ['review', 'github'],
  license: '',
  compatibility: '',
  allowedTools: 'Read Grep',
  frontmatter: {},
}

test('composes portable Agent Skills frontmatter without leaking Depot catalog tags', () => {
  const source = composeArtifactSource('Skill', metadata, '# Workflow')
  assert.match(source, /^---\nname: "repo-triage"\ndescription: "Triage a repository\."/)
  assert.doesNotMatch(source, /tags:/)
  assert.match(source, /allowed-tools: "Read Grep"\n---\n\n# Workflow$/)
})

test('exposes official provider-specific frontmatter per artifact kind', () => {
  const skill = artifactFrontmatterFields('Skill')
  assert.ok(skill.some((field) => field.frontmatterKey === 'license' && field.ecosystem === 'Portable'))
  assert.ok(skill.some((field) => field.frontmatterKey === 'disable-model-invocation' && field.ecosystem === 'Claude'))
  const agent = artifactFrontmatterFields('Agent')
  assert.ok(agent.some((field) => field.frontmatterKey === 'permissionMode'))
  assert.ok(agent.some((field) => field.frontmatterKey === 'mcpServers'))
  const prompt = artifactFrontmatterFields('Prompt')
  assert.deepEqual(prompt.map((field) => field.frontmatterKey), ['argument-hint'])
})

test('serializes provider field shapes as valid YAML flow values', () => {
  const source = composeArtifactSource('Agent', { ...metadata, frontmatter: { maxTurns: '12', background: 'true', skills: 'review rust', mcpServers: '["github"]' } }, '# Review')
  assert.match(source, /maxTurns: 12/)
  assert.match(source, /background: true/)
  assert.match(source, /skills: \["review","rust"\]/)
  assert.match(source, /mcpServers: \["github"\]/)
})

test('Codex custom prompts use filename-derived names and official prompt metadata', () => {
  const source = composeArtifactSource('Prompt', { ...metadata, frontmatter: { 'argument-hint': 'FILE=<path>' } }, '# Draft')
  assert.doesNotMatch(source, /^name:/m)
  assert.match(source, /description: "Triage a repository\."/)
  assert.match(source, /argument-hint: "FILE=<path>"/)
})

test('validates Agent Skills naming and provider field shapes', () => {
  const issues = validateArtifactDraft('Skill', { ...metadata, name: 'Repo--Triage', frontmatter: { background: 'sometimes', metadata: '{broken' } }, '# Workflow')
  assert.ok(issues.some((entry) => entry.field === 'name' && entry.severity === 'error'))
  assert.ok(issues.some((entry) => entry.field === 'frontmatter' && /boolean/.test(entry.message)))
  assert.ok(issues.some((entry) => entry.field === 'frontmatter' && /valid JSON/.test(entry.message)))
})

test('lints JSON-backed artifact bodies', () => {
  const issues = validateArtifactDraft('MCP', metadata, '{broken')
  assert.ok(issues.some((entry) => entry.field === 'content' && entry.severity === 'error'))
})
