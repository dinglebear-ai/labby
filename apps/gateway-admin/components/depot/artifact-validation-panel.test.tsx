import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { ArtifactValidationPanel } from './artifact-validation-panel'
import type { ArtifactMetadata } from '@/lib/editor/artifact-standards'

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

test('skill validation panel renders the reference six checks from draft state and selects their field', async () => {
  const window = installTestDom()
  let selected = ''
  const view = await renderClient(
    <ArtifactValidationPanel
      kind="Skill"
      metadata={metadata}
      content={body}
      issues={[]}
      onField={field => { selected = field }}
    />,
  )
  try {
    assert.equal(view.container.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow'), '5')
    const text = view.container.textContent ?? ''
    assert.match(text, /Validation5 of 6/)
    assert.match(text, /Name is a slug/)
    assert.match(text, /At least two tags/)
    assert.match(text, /No example transcript/)
    const tagButton = Array.from(view.container.querySelectorAll('button')).find(button => button.textContent?.includes('At least two tags'))!
    await act(async () => tagButton.dispatchEvent((new window.MouseEvent('click', { bubbles: true }) as unknown as Event)))
    assert.equal(selected, 'tags')
  } finally {
    await view.unmount()
  }
})
