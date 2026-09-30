import test from 'node:test'
import assert from 'node:assert/strict'

import { BackendGatewayError, gatewayAction } from './gateway-backend-core.ts'

test('gatewayAction surfaces backend request ids on error responses', async () => {
  const originalFetch = globalThis.fetch
  try {
    globalThis.fetch = (async () =>
      new Response(
        JSON.stringify({
          kind: 'internal_error',
          message: 'gateway backend unavailable',
        }),
        {
          status: 500,
          headers: {
            'content-type': 'application/json',
            'x-request-id': 'req-backend-500',
          },
        },
      )) as typeof fetch

    await assert.rejects(
      gatewayAction('gateway.list', {}),
      (error: unknown) => {
        assert.equal(typeof error, 'object')
        assert.ok(error)
        assert.equal((error as BackendGatewayError).name, 'BackendGatewayError')
        assert.equal((error as BackendGatewayError).kind, 'internal_error')
        assert.equal((error as BackendGatewayError).requestId, 'req-backend-500')
        assert.match((error as BackendGatewayError).message, /req-backend-500/)
        return true
      },
    )
  } finally {
    globalThis.fetch = originalFetch
  }
})
