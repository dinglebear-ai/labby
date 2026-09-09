import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { ArtifactFieldIndicator } from './artifact-field-indicator'
import type { ArtifactIssue } from '@/lib/editor/artifact-standards'

test('field indicators reflect only their own issues and errors outrank warnings', () => {
  const warning: ArtifactIssue = { field: 'content', severity: 'warning', message: 'Add a heading.', from: 0, to: 1 }
  const error: ArtifactIssue = { ...warning, severity: 'error', message: 'Body is required.' }
  const html = (issues: ArtifactIssue[]) => renderToStaticMarkup(<ArtifactFieldIndicator field="content" issues={issues} />)
  assert.match(html([]), /aurora-success/)
  assert.match(html([warning]), /aurora-warn/)
  assert.match(html([warning, error]), /aurora-error/)
  assert.match(html([error]), /aria-label="content: Body is required\."/)
  assert.match(html([{ ...error, field: 'name' }]), /aurora-success/)
})
