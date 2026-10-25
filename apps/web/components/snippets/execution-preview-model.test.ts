import test from 'node:test'
import assert from 'node:assert/strict'
import {
  containsSensitiveValue,
  inheritedInputSchema,
  safeReplayDefaults,
} from './execution-preview-model'
import { redactWorkflowValue } from './workflow-redaction'
import { builderInputSpecs } from './builder-inputs'
test('replay omits complete defaults containing credentials including nested arrays', () => {
  assert.deepEqual(
    safeReplayDefaults({
      host: { ty: 'string', default: 'node' },
      token: { ty: 'string', default: 'secret' },
      config: { ty: 'json', default: { servers: [{ password: 'secret' }] } },
    }),
    { host: 'node' },
  )
  assert.equal(containsSensitiveValue('value', { format: 'password' }), true)
})
test('schema-sensitive target aliases redact generic input fields', () => {
  const schema = inheritedInputSchema(
    {
      version: 1,
      steps: [
        {
          id: 'connect',
          tool: 'host::connect',
          dependsOn: [],
          mapping: { credential: '$input.value' },
        },
      ],
    },
    {
      connect: {
        type: 'object',
        properties: { credential: { type: 'string', writeOnly: true } },
      },
    },
  )
  assert.deepEqual(
    redactWorkflowValue({ value: 'secret', host: 'node' }, schema),
    { value: '[redacted]', host: 'node' },
  )
})

test('nested target aliases carry sensitivity to nested source paths', () => {
  const schema = inheritedInputSchema(
    {
      version: 1,
      steps: [
        {
          id: 'connect',
          tool: 'host::connect',
          dependsOn: [],
          mapping: { config: { credential: '$input.settings.value' } },
        },
      ],
    },
    {
      connect: {
        type: 'object',
        properties: {
          config: {
            type: 'object',
            properties: { credential: { format: 'password' } },
          },
        },
      },
    },
  )
  assert.deepEqual(
    redactWorkflowValue(
      { settings: { value: 'secret', host: 'node' } },
      schema,
    ),
    { settings: { value: '[redacted]', host: 'node' } },
  )
})

test('whole aliases carry sensitive descendants through objects and arrays without hiding safe siblings', () => {
  const plan = { version: 1 as const, steps: [{ id: 'connect', tool: 'host::connect', dependsOn: [], mapping: { config: '$input.settings', accounts: '$input.people' } }] }
  const schema = inheritedInputSchema(plan, { connect: { type: 'object', properties: {
    config: { type: 'object', properties: { value: { writeOnly: true }, host: { type: 'string' } } },
    accounts: { type: 'array', items: { type: 'object', properties: { value: { format: 'password' }, label: { type: 'string' } } } },
  } } })
  const input = { settings: { value: 'secret-one', host: 'node' }, people: [{ value: 'secret-two', label: 'safe' }] }
  assert.deepEqual(redactWorkflowValue(input, schema), { settings: { value: '[redacted]', host: 'node' }, people: [{ value: '[redacted]', label: 'safe' }] })
  assert.deepEqual(safeReplayDefaults({ settings: { ty: 'json', default: input.settings }, people: { ty: 'json', default: input.people }, host: { ty: 'string', default: 'node' } }, schema), { host: 'node' })
})

test('complex unresolved alias schemas conservatively omit defaults instead of assuming safe values', () => {
  const plan = { version: 1 as const, steps: [{ id: 'connect', tool: 'host::connect', dependsOn: [], mapping: { config: '$input.settings' } }] }
  for (const config of [{ allOf: [{ properties: { value: { writeOnly: true } } }] }, { $ref: '#/$defs/settings' }, { additionalProperties: true }, { additionalProperties: { writeOnly: true } }]) {
    const schema = inheritedInputSchema(plan, { connect: { type: 'object', properties: { config } } })
    assert.deepEqual(safeReplayDefaults({ settings: { ty: 'json', default: { value: 'private' } } }, schema), {})
    assert.deepEqual(redactWorkflowValue({ settings: { value: 'private' } }, schema), { settings: '[redacted]' })
  }
})

test('closed known object aliases preserve ordinary defaults and visible safe values', () => {
  const plan = { version: 1 as const, steps: [{ id: 'connect', tool: 'host::connect', dependsOn: [], mapping: { config: '$input.settings' } }] }
  const schema = inheritedInputSchema(plan, { connect: { type: 'object', properties: { config: { type: 'object', additionalProperties: false, properties: { host: { type: 'string' } } } } } })
  const settings = { host: 'node' }
  assert.deepEqual(builderInputSpecs({}, { settings }, schema), { settings: { ty: 'json', required: false, default: settings } })
  assert.deepEqual(safeReplayDefaults({ settings: { ty: 'json', default: settings } }, schema), { settings })
  assert.deepEqual(redactWorkflowValue({ settings }, schema), { settings })
})

test('numeric top-level input aliases omit sensitive new, existing and replay defaults', () => {
  const schema = inheritedInputSchema(
    { version: 1, steps: [{ id: 'connect', tool: 'host::connect', dependsOn: [], mapping: { credential: '$input.0' } }] },
    { connect: { type: 'object', properties: { credential: { type: 'string', writeOnly: true } } } },
  )
  const inputs = { '0': { ty: 'string' as const, default: 'synthetic-private' } }
  assert.equal(schema.properties?.['0']?.writeOnly, true)
  assert.deepEqual(builderInputSpecs({}, { '0': 'synthetic-private' }, schema), { '0': { ty: 'string', required: true } })
  assert.deepEqual(builderInputSpecs(inputs, {}, schema), { '0': { ty: 'string', required: true } })
  assert.deepEqual(safeReplayDefaults(inputs, schema), {})
  assert.deepEqual(redactWorkflowValue({ '0': 'synthetic-private' }, schema), { '0': '[redacted]' })
})
