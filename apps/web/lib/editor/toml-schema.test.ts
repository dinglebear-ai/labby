import test from 'node:test'
import assert from 'node:assert/strict'

import { autocompleteTomlDocument, validateTomlDocument } from './toml-schema'

test('flags missing recommended keys for labby.toml', () => {
  const diagnostics = validateTomlDocument('labby.toml', 'title = "Labby"')

  assert.ok(diagnostics.some((item) => item.message.includes('theme')))
})

test('rejects unsupported theme values for labby.toml', () => {
  const diagnostics = validateTomlDocument('labby.toml', 'title = "Labby"\ntheme = "midnight"')

  assert.ok(diagnostics.some((item) => item.message.includes('Unsupported theme')))
})

test('suggests deployment table keys for labby.toml', () => {
  const items = autocompleteTomlDocument('labby.toml')

  assert.ok(items.some((item) => item.label === '[deploy]'))
  assert.ok(items.some((item) => item.label === 'target'))
})

test('resolves known TOML schemas by nested path as well as basename', () => {
  const items = autocompleteTomlDocument('configs/labby.toml')

  assert.ok(items.some((item) => item.label === 'theme'))
})
