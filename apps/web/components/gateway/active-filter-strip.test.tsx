import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'
import { ActiveFilterStrip } from './active-filter-strip'

installTestDom()
test('removing a chip targets only that filter; clearing all is separate', async () => {
  const calls: string[] = []
  const view = await renderClient(<ActiveFilterStrip filters={[{ id: 'status', label: 'Healthy', remove: () => calls.push('status') }, { id: 'transport', label: 'HTTP', remove: () => calls.push('transport') }]} onClear={() => calls.push('all')}/>)
  try {
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Remove HTTP filter"]')!.click())
    assert.deepEqual(calls, ['transport'])
    await act(async () => [...document.querySelectorAll('button')].find(button => button.textContent === 'Clear All')!.click())
    assert.deepEqual(calls, ['transport', 'all'])
  } finally { await view.unmount() }
})
test('no active filters produce no strip', () => {
  assert.equal(renderToStaticMarkup(<ActiveFilterStrip filters={[]} onClear={() => {}}/>), '')
})
