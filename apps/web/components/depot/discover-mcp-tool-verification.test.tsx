import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { DiscoverMcpToolVerification } from './discover-mcp-tool-verification'
installTestDom()
const originalFetch = globalThis.fetch
test.afterEach(() => { globalThis.fetch = originalFetch })
async function settle(check: () => void) { for (let i = 0; i < 50; i++) { try { check(); return } catch { await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) }) } } check() }
for (const success of [true, false]) test(`first-tool ${success ? 'success' : 'failure'} requires explicit named approval`, async () => {
  let calls = 0
  globalThis.fetch = async (_url, init) => {
    const request = JSON.parse(String(init?.body))
    if (request.action === 'setup.mcp.verification.tools') return Response.json({ tools: [{ name: 'version', description: 'Read the server version.', reviewFingerprint: 'a'.repeat(64) }] })
    assert.equal(request.action, 'setup.mcp.verification.call')
    assert.deepEqual(request.params, { name: 'server', expected_url: 'https://example.org/mcp', tool: 'version', expected_fingerprint: 'a'.repeat(64), arguments: {}, approved: true })
    calls++
    return success ? Response.json({ verified: true, server: 'server', tool: 'version' }) : Response.json({ message: 'Tool check failed' }, { status: 502 })
  }
  const view = await renderClient(<DiscoverMcpToolVerification server="server" endpoint="https://example.org/mcp" />)
  try {
    await settle(() => assert.ok(view.container.querySelector('select')))
    const button = [...view.container.querySelectorAll('button')].find(button => button.textContent?.includes('Run selected tool check'))!
    assert.equal(button.disabled, true); assert.equal(calls, 0)
    await act(async () => (view.container.querySelector('input[type=checkbox]') as HTMLInputElement).click())
    await act(async () => button.click())
    assert.equal(calls, 1)
    assert.match(view.container.textContent ?? '', success ? /Tool call verified: version/ : /Tool check failed/)
    assert.equal(button.disabled, true)
    if (!success) assert.doesNotMatch(view.container.textContent ?? '', /Tool call verified/)
  } finally { await view.unmount() }
})
test('required enum inputs are populated and invalid inputs cannot authorize a call', async () => {
  let calls = 0
  globalThis.fetch = async (_url, init) => {
    const request = JSON.parse(String(init?.body))
    if (request.action === 'setup.mcp.verification.tools') return Response.json({ tools: [{ name: 'time', description: 'Read current time.', reviewFingerprint: 'a'.repeat(64), inputSchema: { properties: { timezone: { type: 'string', description: 'Timezone used for the returned time.', enum: ['UTC', 'America/New_York'] } }, required: ['timezone'] } }] })
    calls++
    assert.deepEqual(request.params.arguments, { timezone: 'UTC' })
    return Response.json({ verified: true, server: 'server', tool: 'time' })
  }
  const view = await renderClient(<DiscoverMcpToolVerification server="server" endpoint="https://example.org/mcp" />)
  try {
    await settle(() => assert.ok(view.container.querySelector('[aria-label=timezone]')))
    const select = view.container.querySelector('[aria-label=timezone]') as HTMLSelectElement
    assert.deepEqual([...select.options].map(option => option.value), ['', 'UTC', 'America/New_York'])
    const button = [...view.container.querySelectorAll('button')].find(button => button.textContent?.includes('Run selected tool check'))!
    await act(async () => (view.container.querySelector('input[type=checkbox]') as HTMLInputElement).click())
    assert.equal(button.disabled, true); assert.equal(calls, 0)
    await act(async () => { select.value = 'UTC'; select.dispatchEvent(new Event('change', { bubbles: true })) })
    assert.equal(button.disabled, true)
    await act(async () => (view.container.querySelector('input[type=checkbox]') as HTMLInputElement).click())
    await act(async () => button.click())
    assert.equal(calls, 1)
  } finally { await view.unmount() }
})
