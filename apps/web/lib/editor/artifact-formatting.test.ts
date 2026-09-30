import test from 'node:test'
import assert from 'node:assert/strict'
import { formatArtifactSelection } from './artifact-formatting'

test('formatting preserves unselected draft text and places the cursor after insertion', () => {
  assert.deepEqual(formatArtifactSelection('before word after', 7, 11, 'bold'), { content: 'before **word** after', cursor: 15 })
  assert.equal(formatArtifactSelection('abc', 0, 3, 'code').content, '```\nabc\n```')
  assert.equal(formatArtifactSelection('', 0, 0, 'heading').content, '## Heading')
  assert.equal(formatArtifactSelection('item', 0, 4, 'bullet').content, '- item')
  assert.equal(formatArtifactSelection('step', 0, 4, 'numbered').content, '1. step')
  assert.equal(formatArtifactSelection('text', 0, 4, 'italic').content, '*text*')
  assert.equal(formatArtifactSelection('value', 0, 5, 'inlineCode').content, '`value`')
})
