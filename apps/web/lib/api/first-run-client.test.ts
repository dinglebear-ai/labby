import assert from 'node:assert/strict'
import test from 'node:test'
import { firstRunApi } from './setup-client'

const provider = { configured: true, base_url: 'https://provider.example/v1/', api_key_configured: true, externally_managed: false }
const verified = { provider, models: ['fixture-model'], restart_required: false, agent_verified: false }

test('first-run provider setup uses the protected setup action and never puts credentials in the URL', async () => {
  const originalFetch = globalThis.fetch
  let body: unknown
  let endpoint = ''
  globalThis.fetch = (async (input, init) => {
    endpoint = String(input)
    body = JSON.parse(String(init?.body))
    assert.equal(init?.method, 'POST')
    return new Response(JSON.stringify(verified), { status: 200 })
  }) as typeof fetch
  try {
    const result = await firstRunApi.configureProvider(' https://provider.example/v1 ', ' fixture-key ')
    assert.equal(endpoint, '/v1/setup')
    assert.ok(!endpoint.includes('fixture-key'))
    assert.deepEqual(body, { action: 'onboarding.provider.configure', params: { base_url: 'https://provider.example/v1', api_key: 'fixture-key' } })
    assert.equal(result.agent_verified, false)
  } finally { globalThis.fetch = originalFetch }
})

test('keyless local providers omit a credential rather than manufacturing one', async () => {
  const originalFetch = globalThis.fetch
  let body: unknown
  globalThis.fetch = (async (_input, init) => {
    body = JSON.parse(String(init?.body))
    return new Response(JSON.stringify(verified), { status: 200 })
  }) as typeof fetch
  try {
    await firstRunApi.configureProvider('http://127.0.0.1:1234/v1', '   ')
    assert.deepEqual(body, { action: 'onboarding.provider.configure', params: { base_url: 'http://127.0.0.1:1234/v1' } })
  } finally { globalThis.fetch = originalFetch }
})

test('model verification reuses the stored provider without resending its API key', async () => {
  const originalFetch = globalThis.fetch
  let body: unknown
  globalThis.fetch = (async (_input, init) => {
    body = JSON.parse(String(init?.body))
    return new Response(JSON.stringify(verified), { status: 200 })
  }) as typeof fetch
  try {
    await firstRunApi.models()
    assert.deepEqual(body, { action: 'onboarding.provider.models', params: {} })
  } finally { globalThis.fetch = originalFetch }
})
