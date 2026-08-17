import test from 'node:test'
import assert from 'node:assert/strict'

import { GatewayApiError } from '../../lib/api/gateway-client-core.ts'
import { toolBrowserError } from './tool-browser.tsx'

test('tool browser presents auth, availability, and request-id failures distinctly', () => {
  assert.deepEqual(
    toolBrowserError(new GatewayApiError('expired', 401, 'auth_failed'), 'fallback'),
    { message: 'Sign in to search tools.', status: 401, requestId: undefined },
  )
  assert.deepEqual(
    toolBrowserError(new GatewayApiError('denied', 403, 'forbidden'), 'fallback'),
    { message: 'Administrator access is required.', status: 403, requestId: undefined },
  )
  assert.deepEqual(
    toolBrowserError(new GatewayApiError('boom', 503, 'backend_unreachable', 'req-7'), 'fallback'),
    { message: 'Tools are temporarily unavailable.', status: 503, requestId: 'req-7' },
  )
})
