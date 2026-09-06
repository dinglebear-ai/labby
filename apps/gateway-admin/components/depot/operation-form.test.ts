import assert from 'node:assert/strict'
import test from 'node:test'

import { initialOperationForm, isDestructiveOperation, operationParams } from './operation-form.ts'

test('operation forms coerce canonical JSON schema scalar and collection inputs', () => {
  const properties = {
    enabled: { type: 'boolean', default: true },
    limit: { type: 'integer', minimum: 1, maximum: 10 },
    only: { type: 'array', items: { type: 'string' } },
    candidate: { type: 'object' },
    query: { type: 'string' },
  }
  const initial = initialOperationForm(properties)
  assert.equal(initial.enabled, true)
  assert.deepEqual(operationParams(properties, ['limit', 'candidate'], {
    ...initial, limit: '4', only: 'one, two', candidate: '{"id":"a"}', query: '',
  }), { enabled: true, limit: 4, only: ['one', 'two'], candidate: { id: 'a' } })
})

test('operation forms enforce required and numeric schema constraints', () => {
  assert.throws(() => operationParams({ limit: { type: 'integer', minimum: 1 } }, ['limit'], { limit: '' }), /limit is required/)
  assert.throws(() => operationParams({ limit: { type: 'integer', minimum: 1 } }, ['limit'], { limit: '0' }), /at least 1/)
  assert.throws(() => operationParams({ limit: { type: 'integer' } }, ['limit'], { limit: '1.5' }), /must be an integer/)
  assert.throws(() => operationParams({ ratio: { type: 'number' } }, ['ratio'], { ratio: 'nope' }), /must be a number/)
  assert.throws(() => operationParams({ limit: { type: 'integer', maximum: 10 } }, ['limit'], { limit: '11' }), /at most 10/)
})

test('optional booleans preserve omission semantics until explicitly changed', () => {
  const properties = {
    inherited: { type: 'boolean' },
    enabled: { type: 'boolean', default: true },
    disabled: { type: 'boolean', default: false },
  }
  const initial = initialOperationForm(properties)
  assert.equal(initial.inherited, undefined)
  assert.deepEqual(operationParams(properties, [], initial), { enabled: true, disabled: false })
  assert.deepEqual(operationParams(properties, [], { ...initial, inherited: false }), {
    inherited: false, enabled: true, disabled: false,
  })
  assert.throws(() => operationParams({ confirmed: { type: 'boolean' } }, ['confirmed'], {}), /confirmed is required/)
})

test('operation forms reject malformed collection values', () => {
  assert.throws(() => operationParams({ values: { type: 'array' } }, [], { values: '{"not":"array"}' }), /JSON array or comma-separated list/)
  assert.throws(() => operationParams({ values: { type: 'array' } }, [], { values: '[1,' }), /JSON array or comma-separated list/)
  assert.throws(() => operationParams({ options: { type: 'object' } }, [], { options: '[]' }), /JSON object/)
  assert.throws(() => operationParams({ options: { type: 'object' } }, [], { options: '{broken' }), /JSON object/)
})

test('operation forms enforce typed array members and collection constraints', () => {
  const numbers = { type: 'array' as const, minItems: 2, maxItems: 3, uniqueItems: true, items: { type: 'integer' as const, minimum: 1, maximum: 9 } }
  assert.deepEqual(operationParams({ values: numbers }, [], { values: '1, 2' }), { values: [1, 2] })
  assert.throws(() => operationParams({ values: numbers }, [], { values: '1' }), /at least 2 items/)
  assert.throws(() => operationParams({ values: numbers }, [], { values: '1, 1' }), /unique items/)
  assert.throws(() => operationParams({ values: numbers }, [], { values: '1, nope' }), /item 2 must be an integer/)
  assert.throws(() => operationParams({ values: numbers }, [], { values: '1, 10' }), /item 2 must be at most 9/)
})

test('operation forms enforce string and object constraints', () => {
  assert.throws(() => operationParams({ slug: { type: 'string', minLength: 3 } }, [], { slug: 'ab' }), /at least 3 characters/)
  assert.throws(() => operationParams({ slug: { type: 'string', pattern: '^[a-z]+$' } }, [], { slug: 'ABC' }), /invalid format/)
  assert.throws(() => operationParams({ data: { type: 'object', maxProperties: 1 } }, [], { data: '{"a":1,"b":2}' }), /at most 1 properties/)
})

test('destructive state comes from Depot operation annotations', () => {
  assert.equal(isDestructiveOperation({ destructiveHint: true }), true)
  assert.equal(isDestructiveOperation({ destructiveHint: false }), false)
})
