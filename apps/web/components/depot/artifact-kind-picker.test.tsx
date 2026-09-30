import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { ARTIFACT_KINDS } from '@/lib/editor/artifact-standards'
import { artifactTypeDefinition } from './artifact-type'
import { ArtifactKindPicker } from './artifact-kind-picker'

test('each editor kind uses its shared identity color and a visible labelled glyph', () => {
  for (const kind of ARTIFACT_KINDS) {
    const html = renderToStaticMarkup(<ArtifactKindPicker value={kind} onChange={() => {}} />)
    assert.ok(html.includes(`color:${artifactTypeDefinition(kind).color}`))
    assert.ok(html.includes(`aria-label="Change artifact kind: ${kind}"`))
    assert.match(html, /data-visible-label="1"/)
    assert.match(html, /<svg/)
    assert.doesNotMatch(html, /lucide-file-type/)
  }
})
