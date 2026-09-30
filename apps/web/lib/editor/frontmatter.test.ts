import test from 'node:test'
import assert from 'node:assert/strict'

import { validateClaudeFrontmatter } from './frontmatter'

test('accepts valid claude agent frontmatter', () => {
  const diagnostics = validateClaudeFrontmatter(
    'agents/code-reviewer.md',
    "---\nname: code-reviewer\ndescription: Review code\n---\n\nBody",
  )
  assert.equal(diagnostics.length, 0)
})

test('rejects missing required frontmatter fields', () => {
  const diagnostics = validateClaudeFrontmatter(
    'skills/tdd.md',
    "---\nname: tdd\n---\n\nBody",
  )
  assert.equal(diagnostics.length > 0, true)
})

test('ignores markdown files outside supported claude categories', () => {
  const diagnostics = validateClaudeFrontmatter('README.md', '# Demo')
  assert.deepEqual(diagnostics, [])
})
