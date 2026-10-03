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

test('accepts CRLF delimiters and retains original diagnostic offsets', () => {
  const valid = '---\r\nname: tdd\r\ndescription: Test first\r\n---\r\nBody'
  assert.deepEqual(validateClaudeFrontmatter('skills/tdd.md', valid), [])
  const missing = '---\r\nname: tdd\r\n---\r\nBody'
  assert.equal(validateClaudeFrontmatter('skills/tdd.md', missing)[0].to, missing.indexOf('Body'))
})

test('does not accept a delimiter prefix as a closing delimiter', () => {
  assert.match(validateClaudeFrontmatter('skills/tdd.md', '---\nname: tdd\n---invalid\nBody')[0].message, /Expected YAML/)
})
