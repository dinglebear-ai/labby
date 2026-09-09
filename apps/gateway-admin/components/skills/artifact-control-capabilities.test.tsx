import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'

import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils.tsx'
import { gatewayApi } from '../../lib/api/gateway-client.ts'
import { ArtifactControlPlane } from './artifact-control-plane.tsx'

function services(keys: string[]) {
  return keys.map(key => ({ key, display_name: key, category: 'test', description: '', required_env: [], optional_env: [] }))
}

test('pending and partial capability inventories fail closed; retry enables registered controls', async () => {
  installTestDom()
  const original = gatewayApi.supportedServices
  const originalFetch = globalThis.fetch
  let resolve!: (value: Awaited<ReturnType<typeof gatewayApi.supportedServices>>) => void
  let requests = 0
  gatewayApi.supportedServices = async () => new Promise(done => { resolve = done })
  globalThis.fetch = async () => {
    requests += 1
    return new Response(JSON.stringify({ connections: [], sources: [], jobs: [], bundles: [] }), { status: 200, headers: { 'content-type': 'application/json' } })
  }
  const view = await renderClient(<ArtifactControlPlane />)
  try {
    assert.match(view.container.textContent ?? '', /checking available/i)
    assert.equal(requests, 0)
    await act(async () => { resolve(services(['artifacts', 'sources', 'jobs', 'bundles'])) })
    assert.match(view.container.textContent ?? '', /not available/i)
    assert.equal(requests, 0)
    gatewayApi.supportedServices = async () => services(['artifacts', 'sources', 'jobs', 'bundles', 'uploads'])
    await act(async () => { (view.container.querySelector('button') as HTMLButtonElement).click() })
    assert.match(view.container.textContent ?? '', /Ingest repository/)
    assert.ok(requests > 0, 'authority requests begin only after all services are present')
  } finally {
    await view.unmount()
    gatewayApi.supportedServices = original
    globalThis.fetch = originalFetch
  }
})

test('disabled control-plane services never mount controls or send authority requests', async () => {
  installTestDom()
  const original = gatewayApi.supportedServices
  const originalFetch = globalThis.fetch
  let requests = 0
  gatewayApi.supportedServices = async () => []
  globalThis.fetch = async () => { requests += 1; throw new Error('unexpected authority request') }
  const view = await renderClient(<ArtifactControlPlane />)
  try {
    await act(async () => {})
    assert.match(view.container.textContent ?? '', /not available/i)
    assert.match(view.container.textContent ?? '', /built-in upstream API/i)
    assert.equal(requests, 0)
    assert.equal(view.container.querySelector('input'), null)
  } finally {
    await view.unmount()
    gatewayApi.supportedServices = original
    globalThis.fetch = originalFetch
  }
})

test('capability discovery failure fails closed and offers retry without authority requests', async () => {
  installTestDom()
  const original = gatewayApi.supportedServices
  const originalFetch = globalThis.fetch
  let requests = 0
  gatewayApi.supportedServices = async () => { throw new Error('discovery unavailable') }
  globalThis.fetch = async () => { requests += 1; throw new Error('unexpected authority request') }
  const view = await renderClient(<ArtifactControlPlane />)
  try {
    await act(async () => {})
    assert.match(view.container.textContent ?? '', /unable to check/i)
    assert.match(view.container.textContent ?? '', /retry/i)
    assert.equal(requests, 0)
    assert.equal(view.container.querySelector('input'), null)
  } finally {
    await view.unmount()
    gatewayApi.supportedServices = original
    globalThis.fetch = originalFetch
  }
})
