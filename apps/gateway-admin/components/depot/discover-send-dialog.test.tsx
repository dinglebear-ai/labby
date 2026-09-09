import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'

const source = { connection_id: 'catalog', artifact_id: 'skill-one', revision_id: 'revision-one' }
const response = (data: unknown) => new Response(JSON.stringify(data))

test('Send dialog explains unsupported kinds and never imports automatically', async () => {
  const window = installTestDom()
  for (const name of ['Event', 'NodeFilter', 'HTMLInputElement'] as const) Object.defineProperty(globalThis, name, { value: window[name], configurable: true })
  const { DiscoverSendDialog } = await import('./discover-send-dialog')
  const old = globalThis.fetch
  let calls = 0
  globalThis.fetch = async () => { calls += 1; throw new Error('unexpected') }
  const view = await renderClient(<DiscoverSendDialog target={{ title: 'Example MCP', kind: 'mcp', source }} onOpenChange={() => {}} />)
  try {
    assert.match(document.body.textContent ?? '', /currently connected gateway/)
    assert.match(document.body.textContent ?? '', /supports Skills only/)
    assert.equal([...document.querySelectorAll('button')].some(button => button.textContent?.includes('Activate in this gateway')), false)
    assert.equal(calls, 0)
  } finally {
    globalThis.fetch = old
    await view.unmount()
    await window.happyDOM.close()
  }
})

test('Send dialog requires explicit activation and prevents duplicate clicks while pending', async () => {
  const window = installTestDom()
  for (const name of ['Event', 'NodeFilter', 'HTMLInputElement'] as const) Object.defineProperty(globalThis, name, { value: window[name], configurable: true })
  const { DiscoverSendDialog } = await import('./discover-send-dialog')
  const old = globalThis.fetch
  const actions: string[] = []
  let active = false
  let completeActivation: (() => void) | undefined
  let completed = 0
  globalThis.fetch = async (_url, init) => {
    const { action } = JSON.parse(String(init?.body))
    actions.push(action)
    if (action === 'artifacts.depot_membership') return response({ library_version: 3, items: [{ ...source, status: 'exact_revision_present' }] })
    if (action === 'artifacts.get') return response({ library_version: active ? 4 : 3,
      artifact_id: source.artifact_id, archived: false, materialized: true,
      latest_revision_id: source.revision_id, active_revision_id: active ? source.revision_id : null,
      published_library_version: active ? 4 : 3, allowed_actions: ['artifacts.activate'],
    })
    assert.equal(action, 'artifacts.activate')
    await new Promise<void>(resolve => { completeActivation = resolve })
    active = true
    return response({ outcome: 'committed', artifact_id: source.artifact_id, active_revision_id: source.revision_id, committed_library_version: 4, published_library_version: 4 })
  }
  const view = await renderClient(<DiscoverSendDialog target={{ title: 'Example Skill', kind: 'skill', source }} onOpenChange={() => {}} onActivated={() => { completed += 1 }} />)
  try {
    for (let i = 0; i < 30 && !document.querySelector('[data-visible-label]'); i += 1) await act(async () => { await new Promise(resolve => setTimeout(resolve, 5)) })
    const button = document.querySelector<HTMLButtonElement>('button[data-visible-label]')
    assert.ok(button)
    assert.equal(button.textContent, 'Activate in this gateway')
    assert.equal(actions.includes('artifacts.activate'), false)
    await act(async () => { button.click(); button.click() })
    assert.equal(actions.filter(action => action === 'artifacts.activate').length, 1)
    assert.equal(button.disabled, true)
    assert.equal(button.textContent, 'Activating…')
    assert.ok(completeActivation)
    await act(async () => { completeActivation?.() })
    assert.equal(completed, 1)
    assert.match(document.body.textContent ?? '', /exact Skill revision is active/)
    assert.equal(actions.includes('artifacts.import'), false)
  } finally {
    completeActivation?.()
    globalThis.fetch = old
    await view.unmount()
    await window.happyDOM.close()
  }
})
