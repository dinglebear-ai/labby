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

test('Phoenix MCP App read preserves the resource response contract', async () => {
  const originalFetch = globalThis.fetch
  let body: unknown
  globalThis.fetch = async (_input, init) => {
    body = JSON.parse(String(init?.body))
    return new Response(JSON.stringify({ contents: [{ uri: 'ui://fixture/app.html', mimeType: 'text/html;profile=mcp-app', text: '<main>Fixture</main>', _meta: { ui: { prefersBorder: true } } }] }), { status: 200, headers: { 'Content-Type': 'application/json' } })
  }
  try {
    const result = await phoenixApi.readMcpAppResource('ui://fixture/app.html')
    assert.deepEqual(body, { action: 'phoenix.mcp_app.read', params: { uri: 'ui://fixture/app.html' } })
    assert.equal(result.contents?.[0]?.mimeType, 'text/html;profile=mcp-app')
    assert.deepEqual(result.contents?.[0]?._meta, { ui: { prefersBorder: true } })
  } finally {
    globalThis.fetch = originalFetch
  }
})
