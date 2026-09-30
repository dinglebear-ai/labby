import test from 'node:test'
import assert from 'node:assert/strict'

import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { phoenixApi } from './phoenix-client.ts'

test('Phoenix session listing uses the owner-scoped read action', async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'operator', email: 'operator@example.test' },
    expiresAt: 9999,
    csrfToken: 'csrf-phoenix',
    isAdmin: true,
  })
  const originalFetch = globalThis.fetch
  let requestInit: RequestInit | undefined
  globalThis.fetch = async (_input, init) => {
    requestInit = init
    return new Response(JSON.stringify({
      sessions: [{
        session_id: 'phoenix-opaque',
        title: 'Inspect gateway health',
        preview: 'The gateway is healthy.',
        model: 'gpt-5',
        effort: 'medium',
        message_count: 2,
        turn_status: 'ready',
      }],
    }), { status: 200, headers: { 'Content-Type': 'application/json' } })
  }

  const result = await phoenixApi.list()

  assert.deepEqual(JSON.parse(String(requestInit?.body)), {
    action: 'phoenix.session.list',
    params: {},
  })
  assert.equal(result.sessions[0]?.session_id, 'phoenix-opaque')
  assert.equal(result.sessions[0]?.turn_status, 'ready')
  globalThis.fetch = originalFetch
})
