import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { ArtifactWritingExample } from './artifact-writing-example'
import { ARTIFACT_KINDS } from '@/lib/editor/artifact-standards'

test('writing guidance is explicit, non-interactive, and covers each selectable kind', () => {
  for (const kind of ARTIFACT_KINDS) {
    const html = renderToStaticMarkup(<ArtifactWritingExample kind={kind} />)
    assert.match(html, /Illustrative example — not part of your draft/)
    assert.doesNotMatch(html, /<button|<input|<textarea|<script/)
    assert.match(html, new RegExp(`Writing example for ${kind}`))
  }
  assert.match(renderToStaticMarkup(<ArtifactWritingExample kind="Skill" />), /Cluster open PRs by/)
  assert.match(renderToStaticMarkup(<ArtifactWritingExample kind="Agent" />), /Writing an agent/)
})
