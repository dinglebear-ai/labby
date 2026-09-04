import assert from 'node:assert/strict'
import test from 'node:test'

import { ARTIFACT_TYPES, artifactTypeDefinition } from './artifact-type'

test('every filterable artifact type has a unique color and icon', () => {
  const definitions = ARTIFACT_TYPES.map(artifactTypeDefinition)

  assert.equal(ARTIFACT_TYPES.length, 8)
  assert.equal(new Set(definitions.map((definition) => definition.color)).size, ARTIFACT_TYPES.length)
  assert.equal(new Set(definitions.map((definition) => definition.icon)).size, ARTIFACT_TYPES.length)
  assert.deepEqual(definitions.map((definition) => definition.label), [
    'MCP', 'ACP', 'Agents', 'Skills', 'Commands', 'Plugins', 'Marketplaces', 'Prompts',
  ])
})
