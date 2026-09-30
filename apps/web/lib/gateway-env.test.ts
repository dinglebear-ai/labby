import test from 'node:test'
import assert from 'node:assert/strict'

import { defaultGatewayBearerEnvName, validateBearerTokenEnvName } from './gateway-env.ts'

test('validateBearerTokenEnvName allows blank and valid env var names', () => {
  assert.equal(validateBearerTokenEnvName(''), null)
  assert.equal(validateBearerTokenEnvName('GITHUB_MCP_PAT'), null)
  assert.equal(validateBearerTokenEnvName('LAB_UPSTREAM_TOKEN'), null)
})

test('validateBearerTokenEnvName rejects raw bearer header values', () => {
  assert.equal(
    validateBearerTokenEnvName('Bearer ghp_abc123'),
    'Enter an environment variable name like GITHUB_MCP_PAT, not the token value itself.',
  )
})

test('validateBearerTokenEnvName rejects raw GitHub token values', () => {
  assert.equal(
    validateBearerTokenEnvName('ghp_abc123'),
    'Enter an environment variable name like GITHUB_MCP_PAT, not the token value itself.',
  )
  assert.equal(
    validateBearerTokenEnvName('github_pat_abc123'),
    'Enter an environment variable name like GITHUB_MCP_PAT, not the token value itself.',
  )
})

test('validateBearerTokenEnvName rejects invalid env var syntax', () => {
  assert.equal(
    validateBearerTokenEnvName('MY API TOKEN'),
    'Bearer token env var must be a valid environment variable name.',
  )
  assert.equal(
    validateBearerTokenEnvName('https://example.com'),
    'Bearer token env var must be a valid environment variable name.',
  )
})

test('defaultGatewayBearerEnvName normalizes gateway names', () => {
  assert.equal(defaultGatewayBearerEnvName('github'), 'LAB_GW_GITHUB_AUTH_HEADER')
  assert.equal(
    defaultGatewayBearerEnvName('github-copilot remote'),
    'LAB_GW_GITHUB_COPILOT_REMOTE_AUTH_HEADER',
  )
})

test('defaultGatewayBearerEnvName produces valid env var name when gateway name starts with digit', () => {
  assert.equal(defaultGatewayBearerEnvName('1password'), 'LAB_GW_1PASSWORD_AUTH_HEADER')
  assert.match(defaultGatewayBearerEnvName('1password'), /^[A-Za-z_]/)
})

test('defaultGatewayBearerEnvName falls back to GATEWAY when name is empty or all-special', () => {
  assert.equal(defaultGatewayBearerEnvName(''), 'LAB_GW_GATEWAY_AUTH_HEADER')
  assert.equal(defaultGatewayBearerEnvName('---'), 'LAB_GW_GATEWAY_AUTH_HEADER')
})
