import test from 'node:test'
import assert from 'node:assert/strict'

import { collectEditorDiagnostics } from './diagnostics-registry'

test('returns diagnostics for invalid json targets', async () => {
  const diagnostics = await collectEditorDiagnostics('plugin.json', '{')
  assert.equal(diagnostics.length > 0, true)
})

test('returns diagnostics for invalid toml targets', async () => {
  const diagnostics = await collectEditorDiagnostics('labby.toml', 'title = ')
  assert.equal(diagnostics.length > 0, true)
})

test('returns frontmatter diagnostics for supported markdown files', async () => {
  const diagnostics = await collectEditorDiagnostics('agents/demo.md', '---\nname: demo\n---')
  assert.equal(diagnostics.length > 0, true)
})
