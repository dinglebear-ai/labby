import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { gatewayApi } from '@/lib/api/gateway-client'
import type { Gateway } from '@/lib/types/gateway'
import { DiscoverMcpActivation } from './discover-mcp-activation'

installTestDom()
Object.defineProperty(globalThis, 'self', { value: window, configurable: true })
test('Discover approval gates creation, and failed probe retries the saved server', async () => {
  const originalCreate = gatewayApi.create, originalTest = gatewayApi.test, originalFetch = globalThis.fetch
  globalThis.fetch = async () => Response.json({ tools: [] })
  let creates = 0, probes = 0
  gatewayApi.create = async () => { creates++; return { id: 'saved-server' } as Gateway }
  gatewayApi.test = async () => { probes++; return { success: probes > 1, discovered_tools: 2, message: 'Probe result', error: 'not connected' } }
  const view = await renderClient(<DiscoverMcpActivation artifact={{ providerId: 'public', artifactId: 'a', name: 'server', currentRevisionId: 'r1', mcpConnection: { schemaVersion: 'labby.mcp-connection/v1', revisionId: 'r1', transport: 'http', authentication: 'none', url: 'https://example.org/mcp' } }} />)
  try {
    const button = view.container.querySelector('button')!
    assert.equal(button.disabled, true)
    await act(async () => (view.container.querySelector('input[type=checkbox]') as HTMLInputElement).click())
    await act(async () => button.click())
    assert.match(view.container.textContent ?? '', /Server saved; connection needs attention/)
    await act(async () => button.click())
    assert.equal(creates, 1)
    assert.equal(probes, 2)
    assert.match(view.container.textContent ?? '', /Choose a tool below/)
    assert.match(view.container.querySelector('a')?.getAttribute('href') ?? '', /^\/gateway\/?\?id=saved-server$/)
  } finally { await view.unmount(); gatewayApi.create = originalCreate; gatewayApi.test = originalTest; globalThis.fetch = originalFetch }
})
