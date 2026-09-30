import test from 'node:test'
import assert from 'node:assert/strict'
import {
  artifactValidationSummary,
  skillAuthoringChecks,
  skillAuthoringSummary,
} from './artifact-validation-summary'
import type { ArtifactIssue, ArtifactMetadata } from './artifact-standards'

const metadata: ArtifactMetadata = {
  name: 'repo-triage',
  description: 'Cluster open PRs and issues by subsystem, then draft a triage note per cluster.',
  tags: ['review', 'github'],
  license: '',
  compatibility: '',
  allowedTools: '',
}
const body = `## When to use

Invoke when the user asks to triage, group, or summarize open work in a repository.

## Steps

1. List open PRs and issues with labels and last activity.
2. Cluster by touched subsystem, not by label.
3. For each cluster write: what it is, who owns it, what unblocks it.`

test('generic summary counts fields once even when a field has multiple issues', () => {
  const issue: ArtifactIssue = { field: 'name', severity: 'error', message: 'Invalid name', from: 0, to: 1 }
  assert.deepEqual(artifactValidationSummary([]), { passing: 7, total: 7 })
  assert.deepEqual(artifactValidationSummary([issue, issue]), { passing: 6, total: 7 })
  assert.deepEqual(artifactValidationSummary([issue, { ...issue, field: 'content', severity: 'warning' }]), { passing: 5, total: 7 })
  assert.deepEqual(artifactValidationSummary([{ ...issue, field: 'tags' }]), { passing: 6, total: 7 })
})

test('skill authoring checks match the reference six-check model from real draft data', () => {
  const checks = skillAuthoringChecks(metadata, body)
  assert.deepEqual(checks.map(check => check.label), [
    'Name is a slug',
    'Description is loadable',
    'At least two tags',
    'Body has sections',
    'Body has substance',
    'Has a worked example',
  ])
  assert.deepEqual(skillAuthoringSummary(metadata, body), { checks, passing: 5, total: 6 })
  assert.equal(checks.find(check => check.id === 'example')?.optional, true)
  assert.equal(skillAuthoringSummary({ ...metadata, tags: ['review'] }, body).passing, 4)
  assert.equal(skillAuthoringSummary(metadata, `${body}\n\n## Example\n\nReview two open PRs.`).passing, 6)
})

test('validator errors override a passing skill authoring check so blocking problems stay visible', () => {
  const metadata = { name: 'repo-triage', description: 'Triage open pull requests.', tags: ['review', 'github'], license: '', compatibility: '', allowedTools: '' }
  const content = '## When to use\n\nUse it.\n\n## Steps\n\n1. Read.\n\n' + 'x'.repeat(200)
  const passing = skillAuthoringChecks(metadata, content).find(check => check.id === 'tags')!
  assert.equal(passing.passing, true)
  const overridden = skillAuthoringChecks(metadata, content, [
    { field: 'tags', severity: 'error', message: 'Tags cannot exceed 64 bytes each.', from: 0, to: 0 },
  ]).find(check => check.id === 'tags')!
  assert.equal(overridden.passing, false)
  assert.equal(overridden.description, 'Tags cannot exceed 64 bytes each.')
  // Warnings and already-failing checks keep their authoring description.
  const warned = skillAuthoringChecks(metadata, content, [{ field: 'tags', severity: 'warning', message: 'nit', from: 0, to: 0 }]).find(check => check.id === 'tags')!
  assert.equal(warned.passing, true)
  assert.equal(skillAuthoringSummary(metadata, content, [{ field: 'tags', severity: 'error', message: 'too many', from: 0, to: 0 }]).passing, skillAuthoringSummary(metadata, content).passing - 1)
})
