import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'

test('view options expose sort, density and layout controls', async () => {
  const window = installTestDom()
  for (const name of ['Event', 'NodeFilter', 'HTMLInputElement'] as const) {
    Object.defineProperty(globalThis, name, { value: window[name], configurable: true })
  }
  const { DiscoverViewOptions } = await import('./discover-view-options')
  let layout = 'cards'
  let sort = 'relevance'
  let density = 'comfortable'
  const props = {
    sort: 'relevance' as const, setSort: (value: string) => { sort = value },
    layout: 'cards' as const, setLayout: (value: string) => { layout = value },
    density: 'comfortable' as const, setDensity: (value: string) => { density = value },
  }
  const view = await renderClient(<DiscoverViewOptions {...props} />)
  try {
    const trigger = view.container.querySelector('button')
    assert.ok(trigger)
    await act(async () => { trigger.click() })
    assert.match(document.body.textContent ?? '', /Relevance/)
    assert.match(document.body.textContent ?? '', /Most Installed/)
    assert.match(document.body.textContent ?? '', /Most Starred/)
    assert.equal(document.querySelector('select'), null)
    const list = document.querySelector<HTMLButtonElement>('button[aria-label="List view"]')
    assert.ok(list)
    await act(async () => { list.click() })
    assert.equal(layout, 'list')
    assert.equal(document.querySelector('button[aria-label="Table"]'), null)
    assert.match(document.body.textContent ?? '', /Density/)
    const sortGroup = document.querySelector('[aria-label="Sort retained results"]')
    assert.ok(sortGroup)
    const recent = Array.from(sortGroup.querySelectorAll('button')).find(button => button.textContent === 'Recently Updated')
    assert.ok(recent)
    await act(async () => { recent.click() })
    assert.equal(sort, 'newest')
    const compact = document.querySelector<HTMLButtonElement>('button[aria-label="Compact"]')
    assert.ok(compact)
    await act(async () => { compact.click() })
    assert.equal(density, 'compact')
  } finally {
    await view.unmount()
    await window.happyDOM.close()
  }
})
