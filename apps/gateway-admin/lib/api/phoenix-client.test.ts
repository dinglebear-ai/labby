import test from 'node:test'
import assert from 'node:assert/strict'

import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { comparePhoenixSnapshotVersions, mergePhoenixSnapshot, phoenixApi, phoenixSnapshotVersion } from './phoenix-client.ts'

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

test('Phoenix snapshot versions reject a stale poll after a newer final send', () => {
  const final = phoenixSnapshotVersion({
    messages: [{ role: 'assistant', text: 'done', created_at_ms: 300 }],
    events: [{ method: 'item/completed', params: {}, sequence: 9 }],
  })
  const stalePoll = phoenixSnapshotVersion({
    messages: [],
    events: [{ method: 'item/started', params: {}, sequence: 8 }],
  })
  assert.equal(comparePhoenixSnapshotVersions(stalePoll, final), -1)
  assert.equal(comparePhoenixSnapshotVersions(final, stalePoll), 1)
})

test('Phoenix snapshot versions preserve equal-sequence reloads with newer messages', () => {
  const prior = phoenixSnapshotVersion({
    messages: [{ role: 'user', text: 'go', created_at_ms: 100 }],
    events: [{ method: 'item/completed', params: {}, sequence: 7 }],
  })
  const reload = phoenixSnapshotVersion({
    messages: [
      { role: 'user', text: 'go', created_at_ms: 100 },
      { role: 'assistant', text: 'done', created_at_ms: 200 },
    ],
    events: [{ method: 'item/completed', params: {}, sequence: 7 }],
  })
  assert.equal(comparePhoenixSnapshotVersions(reload, prior), 1)
})

test('Phoenix keeps a successful MCP App when a later poll only reports a transient failure', () => {
  const id = '00000000000000000009:echo:ui://connexin/echo.html'
  const prior = {
    session_id: 's', status: 'ready' as const, messages: [],
    events: [{ method: 'item/completed', params: {}, sequence: 9, mcp_apps: [{
      id, sequence: 9, callId: 'echo', resourceUri: 'ui://connexin/echo.html',
      resource: { contents: [{ uri: 'ui://connexin/echo.html', mimeType: 'text/html', text: '<main>ready</main>' }] },
    }] }],
  }
  const failed = {
    session_id: 's', status: 'ready' as const, messages: [],
    events: [{ method: 'item/completed', params: {}, sequence: 9, mcp_apps: [{
      id, sequence: 9, callId: 'echo', resourceUri: 'ui://connexin/echo.html', errorKind: 'timeout',
    }] }],
  }
  const merged = mergePhoenixSnapshot(prior, failed)
  assert.equal(merged.events?.[0]?.mcp_apps?.[0]?.resource?.contents?.[0]?.text, '<main>ready</main>')
  assert.equal(merged.events?.[0]?.mcp_apps?.[0]?.errorKind, undefined)
})

test('Phoenix allows a failed MCP App to recover when a later snapshot hydrates it successfully', () => {
  const id = '00000000000000000009:echo:ui://connexin/echo.html'
  const failed = {
    session_id: 's', status: 'ready' as const, messages: [],
    events: [{ method: 'item/completed', params: {}, sequence: 9, mcp_apps: [{
      id, sequence: 9, callId: 'echo', resourceUri: 'ui://connexin/echo.html', errorKind: 'timeout',
    }] }],
  }
  const recovered = {
    session_id: 's', status: 'ready' as const, messages: [],
    events: [{ method: 'item/completed', params: {}, sequence: 9, mcp_apps: [{
      id, sequence: 9, callId: 'echo', resourceUri: 'ui://connexin/echo.html',
      resource: { contents: [{ uri: 'ui://connexin/echo.html', mimeType: 'text/html', text: '<main>recovered</main>' }] },
    }] }],
  }
  const merged = mergePhoenixSnapshot(failed, recovered)
  assert.equal(merged.events?.[0]?.mcp_apps?.[0]?.resource?.contents?.[0]?.text, '<main>recovered</main>')
})
