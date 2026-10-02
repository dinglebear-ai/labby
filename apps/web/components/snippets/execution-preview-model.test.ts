import test from 'node:test'
import assert from 'node:assert/strict'
import {
  containsSensitiveValue,
  inheritedInputSchema,
  safeReplayDefaults,
} from './execution-preview-model'
import { redactWorkflowValue } from './workflow-redaction'
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
