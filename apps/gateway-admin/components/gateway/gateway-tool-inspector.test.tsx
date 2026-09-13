import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
installTestDom()
Object.defineProperty(globalThis, 'NodeFilter', { configurable: true, value: window.NodeFilter })
Object.defineProperty(globalThis, 'HTMLInputElement', { configurable: true, value: window.HTMLInputElement })
const originalFetch = globalThis.fetch
test.afterEach(() => { globalThis.fetch = originalFetch })
test('tool inspector loads the exact live definition without executing the tool', async () => {
  const { GatewayToolInspector } = await import('./gateway-tool-inspector')
  const requests: unknown[] = []
  globalThis.fetch = async (input, init) => {
    requests.push([String(input), JSON.parse(String(init?.body))])
    return Response.json({ path: 'cortex::search', id: 'id-search', namespace: 'cortex', name: 'search', description: 'Search retained memory', helper: 'tools.cortex.search', signature: '(query: string)', tags: [], typescript: 'type Input = { query: string };', safety: { read_only: true } })
  }
  const view = await renderClient(<GatewayToolInspector target="cortex::search" onClose={() => {}} />)
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)) })
  assert.deepEqual(requests, [['/v1/gateway/codemode/tools/describe', { target: 'cortex::search' }]])
  assert.match(document.body.textContent ?? '', /type Input = \{ query: string \};/)
  assert.match(document.body.textContent ?? '', /Read only/)
  await view.unmount()
})
