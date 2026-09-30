import assert from 'node:assert/strict'
import test from 'node:test'

import {
  assertNonOverlappingDocsPaths,
  sectionForPath,
  shouldIncludeDoc,
  statusFromMarkdown,
  titleFromMarkdown,
} from './sync-docs.mjs'

test('includes canonical markdown and excludes non-product documentation', () => {
  assert.equal(shouldIncludeDoc('README.md'), true)
  assert.equal(shouldIncludeDoc('services/GATEWAY.md'), true)
  assert.equal(shouldIncludeDoc('generated/action-catalog.md'), true)
  assert.equal(shouldIncludeDoc('archive/README.md'), false)
  assert.equal(shouldIncludeDoc('sessions/2026-09-15.md'), false)
  assert.equal(shouldIncludeDoc('superpowers/research.md'), false)
  assert.equal(shouldIncludeDoc('CLAUDE.md'), false)
  assert.equal(shouldIncludeDoc('../SECRET.md'), false)
  assert.equal(shouldIncludeDoc('/tmp/SECRET.md'), false)
  assert.equal(shouldIncludeDoc('design//SECRET.md'), false)
  assert.equal(shouldIncludeDoc('design/CLAUDE_CODE_AURORA_THEME.md'), true)
})

test('rejects source and output directories that overlap', () => {
  assert.throws(() => assertNonOverlappingDocsPaths('/tmp/docs', '/tmp/docs'), /must not overlap/)
  assert.throws(() => assertNonOverlappingDocsPaths('/tmp/docs', '/tmp/docs/out'), /must not overlap/)
  assert.throws(() => assertNonOverlappingDocsPaths('/tmp/docs', '/tmp/docs/..cache'), /must not overlap/)
  assert.throws(() => assertNonOverlappingDocsPaths('/tmp/docs/source', '/tmp/docs'), /must not overlap/)
  assert.doesNotThrow(() => assertNonOverlappingDocsPaths('/tmp/docs', '/tmp/docs-output'))
})

test('derives display metadata from the markdown and path', () => {
  assert.equal(titleFromMarkdown('# Gateway\n\nBody', 'services/GATEWAY.md'), 'Gateway')
  assert.equal(titleFromMarkdown('Body only', 'runtime/reverse-proxy.md'), 'Reverse Proxy')
  assert.equal(sectionForPath('README.md'), 'Overview')
  assert.equal(sectionForPath('access-control/README.md'), 'Access Control')
})

test('extracts explicit document status and labels noncanonical families', () => {
  assert.equal(
    statusFromMarkdown('---\nstatus: "historical-plan"\n---\n# Plan', 'plans/x.md'),
    'historical-plan',
  )
  assert.equal(statusFromMarkdown("---\nstatus: 'active'\n---\n# Service", 'services/x.md'), 'active')
  assert.equal(statusFromMarkdown('# Generated', 'generated/action-catalog.md'), 'generated')
  assert.equal(statusFromMarkdown('# Plan', 'plans/new/README.md'), 'plan')
  assert.equal(statusFromMarkdown('# Feature', 'features/new/README.md'), 'feature')
  assert.equal(statusFromMarkdown('# Canonical', 'runtime/CONFIG.md'), null)
  assert.throws(
    () => statusFromMarkdown('---\nstatus: ' + 'x'.repeat(81) + '\n---\n# Bad', 'design/bad.md'),
    /exceeds 80 characters/,
  )
})
