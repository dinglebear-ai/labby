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
  await browserApi.setSessionEnabled('session-1', true)
  await browserApi.revoke('browser-1')

  assert.deepEqual(actions, [
    { action: 'browser.pairing.approve', params: { pairing_id: 'pair-1' } },
    { action: 'browser.session.enable', params: { session_id: 'session-1', enabled: true } },
    { action: 'browser.revoke', params: { browser_id: 'browser-1' } },
  ])
})
