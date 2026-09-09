import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { DevContainersHero } from './dev-containers-hero'

test('container hero preserves its visible create action and derives only available statistics', async () => {
  installTestDom()
  let opened = 0
  const view = await renderClient(<DevContainersHero rows={[
    ['Ready', 'base', '', '', '38 pulls'], ['Ready', 'rust', '', '', '12 pulls'], ['Building', 'edge', '', '', 'Layer 4/7'],
  ]} onCreate={() => opened++}/> )
  try {
    const button = view.container.querySelector('button')!
    assert.equal(button.textContent, 'New Container')
    assert.equal(button.dataset.visibleLabel, '1')
    await act(async () => button.click())
    assert.equal(opened, 1)
    assert.match(view.container.textContent ?? '', /1 image building/)
    for (const [label, value] of [['Images', '2published'], ['Pulls', '50recorded'], ['Members', '—unavailable'], ['Building', '1edge']]) {
      assert.equal(view.container.querySelector(`[data-console-hero-stat="${label}"] [data-console-hero-stat-value]`)?.textContent, value)
    }
    assert.doesNotMatch(view.container.textContent ?? '', /this month|9 running/)
  } finally { await view.unmount() }
})
