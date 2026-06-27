import test from 'node:test'
import assert from 'node:assert/strict'

import { testResultFromProbe } from './gateway-test-result.ts'

test('testResultFromProbe treats missing prompt discovery as a successful connection', () => {
  const result = testResultFromProbe(
    {
      name: 'synapse-stdio',
      tool_count: 12,
      resource_count: 3,
      prompt_count: 0,
      last_error: 'failed to list prompts from upstream: Method not found',
    },
    {
      connected: true,
      healthy: true,
      last_error: 'failed to list prompts from upstream: Method not found',
    },
    undefined,
  )

  assert.equal(result.success, true)
  assert.equal(result.severity, 'success')
  assert.equal(result.message, 'Connection test passed')
  assert.equal(result.detail, undefined)
  assert.equal(result.error, undefined)
})

test('testResultFromProbe keeps real connection failures as failures', () => {
  const result = testResultFromProbe(
    {
      name: 'swag',
      tool_count: 0,
      resource_count: 0,
      prompt_count: 0,
      last_error: 'Auth required by upstream server',
    },
    {
      connected: false,
      healthy: false,
      last_error: 'Auth required by upstream server',
    },
    'Authentication is required by https://gateway.example.com/mcp. Configure `bearer_token_env` with a valid upstream token, then reload this gateway.',
  )

  assert.equal(result.success, false)
  assert.equal(result.severity, 'failure')
  assert.equal(result.message, 'Connection test failed')
  assert.equal(
    result.error,
    'Authentication is required by https://gateway.example.com/mcp. Configure `bearer_token_env` with a valid upstream token, then reload this gateway.',
  )
})

test('testResultFromProbe treats missing resource discovery as a successful connection', () => {
  const result = testResultFromProbe(
    {
      name: 'chrome-dev-tools',
      tool_count: 29,
      resource_count: 0,
      prompt_count: 0,
      last_error: 'failed to list resources from upstream: Method not found',
    },
    {
      connected: true,
      healthy: true,
      last_error: 'failed to list resources from upstream: Method not found',
    },
    undefined,
  )

  assert.equal(result.success, true)
  assert.equal(result.severity, 'success')
  assert.equal(result.message, 'Connection test passed')
  assert.equal(result.detail, undefined)
  assert.equal(result.error, undefined)
})
