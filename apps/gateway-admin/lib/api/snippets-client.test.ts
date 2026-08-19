import test from 'node:test'
import assert from 'node:assert/strict'

import { snippetsApi } from './snippets-client'

test('snippets client posts list action to the snippets endpoint', async () => {
  let requestUrl = ''
  let requestBody: unknown
  globalThis.fetch = (async (input, init) => {
    requestUrl = String(input)
    requestBody = JSON.parse(String(init?.body ?? '{}'))
    return new Response(JSON.stringify({ snippets: [] }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch

  const snippets = await snippetsApi.list()

  assert.deepEqual(snippets, [])
  assert.equal(requestUrl, '/v1/snippets')
  assert.deepEqual(requestBody, { action: 'snippets.list', params: {} })
})

test('snippets client sends validate body params', async () => {
  let requestBody: unknown
  globalThis.fetch = (async (_input, init) => {
    requestBody = JSON.parse(String(init?.body ?? '{}'))
    return new Response(JSON.stringify({ valid: true, name: 'draft', mode: 'body' }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch

  const result = await snippetsApi.validate('draft', 'async () => ({ ok: true })')

  assert.equal(result.valid, true)
  assert.deepEqual(requestBody, {
    action: 'snippets.validate',
    params: {
      name: 'draft',
      body: 'async () => ({ ok: true })',
    },
  })
})

test('snippets client sends create body and metadata', async () => {
  let requestBody: unknown
  globalThis.fetch = (async (_input, init) => {
    requestBody = JSON.parse(String(init?.body ?? '{}'))
    return new Response(JSON.stringify({
      name: 'fleet-health',
      description: 'Check the fleet',
      tags: [],
      source: 'user',
      path: '/tmp/fleet-health.md',
      shadowed: false,
    }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch

  const result = await snippetsApi.create({
    name: 'fleet-health',
    description: 'Check the fleet',
    body: 'async () => ({ ok: true })',
  })

  assert.equal(result.name, 'fleet-health')
  assert.deepEqual(requestBody, {
    action: 'snippets.create',
    params: {
      name: 'fleet-health',
      description: 'Check the fleet',
      body: 'async () => ({ ok: true })',
    },
  })
})
