import test from 'node:test'
import assert from 'node:assert/strict'

import { browserApi } from './browser-client'

test('browser client loads the operator lifecycle from the browser endpoint', async () => {
  const actions: unknown[] = []
  globalThis.fetch = (async (input, init) => {
    assert.equal(String(input), '/v1/browser')
    const body = JSON.parse(String(init?.body ?? '{}'))
    actions.push(body)
    const payload = body.action === 'browser.list' ? { browsers: [] }
      : body.action === 'browser.pairing.list' ? { pairings: [] }
        : { sessions: [] }
    return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } })
  }) as typeof fetch

  assert.deepEqual(await browserApi.list(), [])
  assert.deepEqual(await browserApi.pairings(), [])
  assert.deepEqual(await browserApi.sessions(), [])
  assert.deepEqual(actions, [
    { action: 'browser.list', params: {} },
    { action: 'browser.pairing.list', params: {} },
    { action: 'browser.sessions', params: {} },
  ])
})

test('browser client preserves exact consent and identity mutation parameters', async () => {
  const actions: unknown[] = []
  globalThis.fetch = (async (_input, init) => {
    actions.push(JSON.parse(String(init?.body ?? '{}')))
    return new Response('{}', { status: 200, headers: { 'content-type': 'application/json' } })
  }) as typeof fetch

  await browserApi.approvePairing('pair-1')
  await browserApi.setSessionEnabled('session-1', true, 'reviewed-digest')
  await browserApi.revoke('browser-1')

  assert.deepEqual(actions, [
    { action: 'browser.pairing.approve', params: { pairing_id: 'pair-1' } },
    { action: 'browser.session.enable', params: { session_id: 'session-1', enabled: true, catalog_digest: 'reviewed-digest' } },
    { action: 'browser.revoke', params: { browser_id: 'browser-1' } },
  ])
})


test('browser client loads exact catalog details before presenting session consent', async () => {
  const actions: string[] = []
  const original = globalThis.fetch
  globalThis.fetch = (async (_input, init) => {
    const body = JSON.parse(String(init?.body ?? '{}'))
    actions.push(body.action)
    const payload = body.action === 'browser.sessions'
      ? { sessions: [{ id: 'session-1', tool_count: 1 }], next_cursor: null }
      : { id: 'session-1', catalog_digest: 'reviewed-digest', tools: [{ name: 'echo' }] }
    return new Response(JSON.stringify(payload), { status: 200, headers: { 'Content-Type': 'application/json' } })
  }) as typeof fetch
  try {
    const sessions = await browserApi.sessions()
    assert.equal(sessions[0]?.catalog_digest, 'reviewed-digest')
    assert.deepEqual(actions, ['browser.sessions', 'browser.session.get'])
  } finally {
    globalThis.fetch = original
  }
})
