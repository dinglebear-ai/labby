import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'
import { DiscoverFeedTabs } from './discover-feed-tabs'

test('reference feed tabs preserve order without inventing unsupported rankings', async () => {
  const window = installTestDom()
  const selections: Array<string | undefined> = []
  const view = await renderClient(<DiscoverFeedTabs onSelect={value => selections.push(value)} />)
  try {
    const buttons = [...view.container.querySelectorAll('button')]
    assert.deepEqual(buttons.map(button => button.textContent), ['Trending', 'New', 'Popular', 'Bundled', 'Hot Forks', 'Curated'])
    assert.ok(buttons.every(button => button.hasAttribute('data-visible-label')))
    assert.equal(buttons.filter(button => button.getAttribute('aria-disabled') === 'true').length, 5)
    await act(async () => { buttons[0].click(); buttons[2].click() })
    assert.equal(selections.length, 0)
    await act(async () => { buttons[1].click() })
    assert.equal(selections.length, 1)
    assert.equal(selections[0], 'new')
    await view.rerender(<DiscoverFeedTabs selected="new" onSelect={value => selections.push(value)} />)
    const selected = view.container.querySelector<HTMLButtonElement>('[aria-pressed="true"]')
    assert.equal(selected?.textContent, 'New')
    await act(async () => { selected?.click() })
    assert.deepEqual(selections, ['new', undefined])
  } finally { await view.unmount(); await window.happyDOM.close() }
})
