import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { AgentsHero } from './agents-hero'

test('Agents header exposes its action and derives counts from its session rows', async () => {
  installTestDom()
  let opened = 0
  const view = await renderClient(<AgentsHero rows={ [['Running'], ['Completed'], ['Failed'], ['Running']] } onCreate={() => opened++} />)
  try {
    const action = view.container.querySelector('button')!
    assert.equal(action.textContent, 'New Session')
    assert.equal(action.dataset.visibleLabel, '1')
    await act(async () => action.click())
    assert.equal(opened, 1)
    assert.match(view.container.textContent ?? '', /2 running/)
    const stats = view.container.querySelector('[aria-label="Agent statistics"]')!
    assert.equal(stats.querySelectorAll('svg').length, 0)
    assert.match(stats.textContent ?? '', /Running2sessionsCompleted1sessionsFailed1sessions/)
    assert.match(stats.textContent ?? '', /Median—unavailable/)
  } finally { await view.unmount() }
})
