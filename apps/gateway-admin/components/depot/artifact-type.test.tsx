import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { ARTIFACT_TYPES, ArtifactTypeMark, artifactTypeDefinition } from './artifact-type'
import { discoverKindPresentation } from './discover-kind-presentation'

test('filter categories preserve labels while mock families share their intended colors', () => {
  const definitions = ARTIFACT_TYPES.map(artifactTypeDefinition)

  assert.equal(ARTIFACT_TYPES.length, 8)
  assert.equal(new Set(definitions.map((definition) => definition.icon)).size, ARTIFACT_TYPES.length)
  assert.deepEqual(definitions.map((definition) => definition.label), [
    'MCP', 'ACP', 'Agents', 'Skills', 'Commands', 'Plugins', 'Marketplaces', 'Prompts',
  ])
})

test('all reference kinds share colors and glyphs across Library and Discover', () => {
  for (const kind of ['mcp', 'acp', 'agent', 'skill', 'command', 'plugin', 'prompt', 'hook', 'extension', 'loadout', 'snippet']) {
    const library = artifactTypeDefinition(kind)
    const discover = discoverKindPresentation(kind)
    assert.equal(library.icon, discover.icon)
    assert.equal(library.color, discover.color)
    assert.deepEqual(library.iconStyle, discover.iconStyle)
  }
})

test('compact marks use reference table geometry and singular kind labels', () => {
  const html = renderToStaticMarkup(<ArtifactTypeMark artifact={{ id: 'one', kind: 'loadout' }} compact />)
  assert.match(html, /size-\[18px\]/)
  assert.match(html, /rounded-\[5px\]/)
  assert.match(html, />Loadout</)
  assert.doesNotMatch(html, />Loadouts</)
  assert.equal(artifactTypeDefinition('future').color, 'var(--aurora-text-muted)')
  for (const kind of ['constructor', '__proto__', 'toString']) {
    assert.equal(artifactTypeDefinition(kind).color, 'var(--aurora-text-muted)')
    assert.doesNotThrow(() => renderToStaticMarkup(<ArtifactTypeMark artifact={{ id: 'unknown', kind }} />))
  }
})
