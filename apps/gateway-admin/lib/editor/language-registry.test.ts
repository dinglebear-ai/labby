import test from 'node:test'
import assert from 'node:assert/strict'

import { getEditorDocumentConfig } from './language-registry'

test('maps supported file paths to the correct editor language', () => {
  assert.equal(getEditorDocumentConfig('plugin.json').language, 'json')
  assert.equal(getEditorDocumentConfig('skills/review.md').language, 'markdown')
  assert.equal(getEditorDocumentConfig('scripts/setup.sh').language, 'bash')
  assert.equal(getEditorDocumentConfig('config/labby.toml').language, 'toml')
})

test('uses dotenv highlighting for .env files', () => {
  assert.equal(getEditorDocumentConfig('Apprise/.env').language, 'dotenv')
})

test('treats .md agent skill and command files as markdown plus frontmatter validation', () => {
  const agent = getEditorDocumentConfig('agents/code-reviewer.md')
  const skill = getEditorDocumentConfig('skills/tdd.md')
  const command = getEditorDocumentConfig('commands/code-review.md')

  assert.equal(agent.language, 'markdown')
  assert.equal(skill.language, 'markdown')
  assert.equal(command.language, 'markdown')
  assert.equal(typeof agent.validate, 'function')
  assert.equal(typeof skill.validate, 'function')
  assert.equal(typeof command.validate, 'function')
})

test('attaches JSON schema support to known JSON targets', () => {
  const config = getEditorDocumentConfig('plugin.json')
  assert.equal(config.language, 'json')
  assert.equal(typeof config.autocomplete, 'function')
  assert.equal(typeof config.validate, 'function')
})

test('attaches TOML validation autocomplete to known TOML targets', () => {
  const config = getEditorDocumentConfig('labby.toml')
  assert.equal(config.language, 'toml')
  assert.equal(typeof config.autocomplete, 'function')
  assert.equal(typeof config.validate, 'function')
})

test('returns no-op diagnostics for plain text files', () => {
  const config = getEditorDocumentConfig('notes.txt')
  assert.equal(config.language, 'text')
  assert.equal(config.validate, undefined)
})
