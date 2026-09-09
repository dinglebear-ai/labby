import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'

test('Fork dialog fails closed without an exact source and never sends a mutation', async () => {
  const window = installTestDom()
  for (const name of ['Event', 'NodeFilter', 'HTMLInputElement'] as const) {
    Object.defineProperty(globalThis, name, { value: window[name], configurable: true })
  }
  const original = globalThis.fetch
  let requests = 0
  globalThis.fetch = async () => { requests++; throw new Error('unexpected request') }
  const { DiscoverForkDialog } = await import('./discover-fork-dialog')
  const view = await renderClient(<DiscoverForkDialog target={{ title: 'Example Artifact' }} onOpenChange={() => {}} />)
  try {
    assert.match(document.body.textContent ?? '', /configured Depot connection and exact revision/)
    assert.match(document.body.textContent ?? '', /Nothing is imported or activated/)
    assert.equal([...document.querySelectorAll('button')].some(button => button.textContent === 'Create hosted fork'), false)
    assert.equal(requests, 0)
  } finally { await view.unmount(); globalThis.fetch = original; await window.happyDOM.close() }
})
