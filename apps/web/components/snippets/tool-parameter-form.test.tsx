import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act, useState } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import type { ParameterSchema } from './tool-parameter-model'

async function waitFor(assertion: () => void) {
  const deadline = Date.now() + 2000; let last: unknown
  while (Date.now() < deadline) { try { assertion(); return } catch (error) { last = error }; await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)) }) }; throw last
}

test('schema form loads authorized tool description and renders typed required fields with JSON escape hatch', async () => {
  const window = installTestDom()
  const { ToolParameterForm } = await import('./tool-parameter-form')
  let fetched: unknown; let loaded: ParameterSchema | undefined; let mapping = '{}'
  globalThis.fetch = (async (_url, init) => {
    fetched = JSON.parse(String(init?.body))
    return new Response(JSON.stringify({ path: 'host.status', id: 'host::status', namespace: 'host', name: 'status', description: 'Check status', helper: 'host.status', signature: '()', tags: [], input_schema: { type: 'object', required: ['host'], properties: { host: { type: 'string', description: 'Host alias' }, count: { type: 'integer', minimum: 1 }, verbose: { type: 'boolean' }, mode: { type: 'string', enum: ['brief', 'full'] } } } }), { headers: { 'content-type': 'application/json' } })
  }) as typeof fetch
  function Harness() { const [value, setValue] = useState('{}'); return <ToolParameterForm tool="host::status" index={0} value={value} inputs={{ hostInput: 'node' }} onChange={(next) => { mapping = next; setValue(next) }} onSchema={(schema) => { loaded = schema }} /> }
  const view = await renderClient(<Harness />)
  try {
    await act(async () => { view.container.querySelector('button')!.click() })
    await waitFor(() => assert.ok(view.container.querySelector('#tool-0-host')))
    assert.deepEqual(fetched, { target: 'host::status' })
    assert.deepEqual(loaded?.required, ['host'])
    assert.match(view.container.textContent ?? '', /host \*|Host alias/)
    assert.equal(view.container.querySelector('#tool-0-count')?.getAttribute('type'), 'number')
    assert.equal(view.container.querySelectorAll('[role="combobox"]').length >= 6, true)
    const host = view.container.querySelector('#tool-0-host')!
    const key = Object.keys(host).find((key) => key.startsWith('__reactProps$'))!
    await act(async () => { (host as unknown as Record<string, { onChange: (event: { target: { value: string } }) => void }>)[key].onChange({ target: { value: 'node-a' } }) })
    assert.deepEqual(JSON.parse(mapping), { host: 'node-a' })
    const source = view.container.querySelector('[aria-label="Value source for host::status host"]')!
    await act(async () => source.dispatchEvent(new window.PointerEvent('pointerdown', { bubbles: true, button: 0, pointerType: 'mouse' }) as unknown as Event))
    await waitFor(() => assert.ok(Array.from(document.body.querySelectorAll('[role="option"]')).some((entry) => entry.textContent?.includes('Snippet input: hostInput'))))
    const option = Array.from(document.body.querySelectorAll('[role="option"]')).find((entry) => entry.textContent?.includes('Snippet input: hostInput'))!
    await act(async () => option.dispatchEvent(new window.KeyboardEvent('keydown', { bubbles: true, key: 'Enter' }) as unknown as Event))
    await waitFor(() => assert.deepEqual(JSON.parse(mapping), { host: '$input.hostInput' }))
    const advanced = Array.from(view.container.querySelectorAll('button')).find((button) => button.textContent === 'Advanced JSON')!
    await act(async () => advanced.click())
    assert.ok(view.container.querySelector('#tool-mapping-0'))
  } finally { await view.unmount() }
})
