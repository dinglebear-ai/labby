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
})

test('destructive state comes from Depot operation annotations', () => {
  assert.equal(isDestructiveOperation({ destructiveHint: true }), true)
  assert.equal(isDestructiveOperation({ destructiveHint: false }), false)
})
