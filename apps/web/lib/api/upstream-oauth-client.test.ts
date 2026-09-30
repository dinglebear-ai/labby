import test from 'node:test'
import assert from 'node:assert/strict'

import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { upstreamOauthApi } from './upstream-oauth-client.ts'

test('upstreamOauthApi keeps hosted session auth and omits bearer headers', async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 42,
    csrfToken: 'csrf-123',
  })

  const originalFetch = globalThis.fetch

  try {
    globalThis.fetch = (async (input, init) => {
      assert.equal(String(input), '/v1/gateway/oauth/upstreams')
      assert.equal(init?.credentials, 'include')
      const headers = new Headers(init?.headers)
      assert.equal(headers.get('Authorization'), null)
      assert.equal(headers.get('x-csrf-token'), 'csrf-123')

      return new Response(JSON.stringify([]), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      })
    }) as typeof fetch

    assert.deepEqual(await upstreamOauthApi.listUpstreams(), [])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('upstreamOauthApi confirms upstream oauth probes', async () => {
  const originalFetch = globalThis.fetch

  try {
    globalThis.fetch = (async (input, init) => {
      assert.equal(String(input), '/v1/gateway/oauth/probe')
      assert.equal(init?.method, 'POST')
      assert.deepEqual(JSON.parse(String(init?.body)), {
        url: 'https://fixture.example/mcp',
        upstream: 'fixture',
        confirm: true,
      })

      return new Response(
        JSON.stringify({
          upstream: 'fixture.example-mcp',
          url: 'https://fixture.example/mcp',
          oauth_discovered: false,
        }),
        {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        },
      )
    }) as typeof fetch

    await upstreamOauthApi.probe('https://fixture.example/mcp', undefined, 'fixture')
  } finally {
    globalThis.fetch = originalFetch
  }
})
