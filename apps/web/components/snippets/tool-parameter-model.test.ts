import test from 'node:test'
import assert from 'node:assert/strict'
import { guidedSchemaSupported, mappedParameterError, objectSchema } from './tool-parameter-model'

const schema = { type: 'object', required: ['query', 'count'], additionalProperties: false, properties: { query: { type: 'string', minLength: 2 }, count: { type: 'integer', minimum: 1, maximum: 5 }, mode: { type: 'string', enum: ['brief', 'full'] }, flag: { type: 'boolean' } } }

test('schema mappings enforce required types bounds and enum choices', () => {
  assert.match(mappedParameterError(schema, {}, {})!, /Required parameter "query"/)
  assert.match(mappedParameterError(schema, { query: 'ok', count: 1.5 }, {})!, /integer/)
  assert.match(mappedParameterError(schema, { query: 'ok', count: 9 }, {})!, /at most 5/)
  assert.match(mappedParameterError(schema, { query: 'x', count: 1 }, {})!, /at least 2 characters/)
  assert.match(mappedParameterError(schema, { query: 'ok', count: 1, mode: 'other' }, {})!, /allowed values/)
  assert.match(mappedParameterError(schema, { query: 'ok', count: 1, surprise: true }, {})!, /Unknown parameter/)
  assert.match(mappedParameterError(schema, { query: '$input.toString', count: 1 }, {})!, /Unknown snippet input/)
  assert.match(mappedParameterError(schema, { query: 'ok', count: 1, toString: 'x' }, {})!, /Unknown parameter/)
  assert.equal(mappedParameterError(schema, { query: '$input.q', count: '$input.n', flag: false }, { q: 'hello', n: 3 }), undefined)
})

test('complex tool schemas retain advanced JSON rather than inventing fields', () => {
  assert.equal(guidedSchemaSupported(schema), true)
  assert.equal(guidedSchemaSupported({ ...schema, properties: { complex: { oneOf: [{ type: 'string' }, { type: 'integer' }] } } }), false)
  assert.equal(objectSchema({ type: 'array' }), undefined)
})
