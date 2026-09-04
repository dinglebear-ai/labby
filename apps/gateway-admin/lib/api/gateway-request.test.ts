import test from 'node:test'
import assert from 'node:assert/strict'

import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { confirmGatewayParams, gatewayHeaders, gatewayRequestInit } from './gateway-request.ts'

test('gatewayRequestInit never sends bearer headers even if legacy bearer inputs are supplied', () => {
  const init = gatewayRequestInit('gateway.list', {}, 'dev-token', undefined, true)

  assert.equal(init.credentials, 'include')
  assert.equal((init.headers as Record<string, string>).Authorization, undefined)
  assert.equal(init.method, 'POST')
})

test('gatewayRequestInit keeps credentialed requests when a token is present without standalone bearer mode', () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 42,
    csrfToken: 'csrf-123',
  })
  const init = gatewayRequestInit('gateway.list', {}, 'dev-token', undefined, false)

  assert.equal(init.credentials, 'include')
  assert.equal((init.headers as Record<string, string>).Authorization, undefined)
  assert.equal((init.headers as Record<string, string>)['x-csrf-token'], 'csrf-123')
})

test('gatewayHeaders omits authorization when no token is provided', () => {
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const headers = gatewayHeaders(undefined) as Record<string, string>

  assert.equal(headers['Content-Type'], 'application/json')
  assert.equal('Authorization' in headers, false)
})

test('gatewayRequestInit keeps credentialed requests for session-auth setups', () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 42,
    csrfToken: 'csrf-123',
  })
  const init = gatewayRequestInit('gateway.list', {}, undefined)

  assert.equal(init.credentials, 'include')
  assert.equal((init.headers as Record<string, string>)['Content-Type'], 'application/json')
  assert.equal((init.headers as Record<string, string>)['x-csrf-token'], 'csrf-123')
})

test('gatewayRequestInit forwards the project bound to the browser session', () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 42,
    csrfToken: 'csrf-123',
    projectId: 'project-42',
  })

  const init = gatewayRequestInit('artifacts.list', {})
  const headers = init.headers as Record<string, string>

  assert.equal(headers['x-labby-project-id'], 'project-42')
  assert.equal(headers['x-csrf-token'], 'csrf-123')
})

test('confirmGatewayParams marks destructive gateway mutations for explicit confirmation', () => {
  assert.deepEqual(confirmGatewayParams({ id: 'gateway_beta' }), {
    confirm: true,
    id: 'gateway_beta',
  })
})
