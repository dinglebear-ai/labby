import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { TasksHero } from './tasks-hero'

test('task hero keeps New Task visible and renders text-only statistics', async () => {
  installTestDom()
  let creates = 0
  const rows = [['Armed', 'Audit', '', '', '', '', 'passed'], ['Paused', 'Review', '', '', '', '', 'failed']]
  const view = await renderClient(<TasksHero rows={rows} onCreate={() => creates++} />)
  try {
    const button = view.container.querySelector<HTMLButtonElement>('button')!
    assert.equal(button.textContent, 'New Task')
    assert.equal(button.dataset.visibleLabel, '1')
    assert.match(button.className, /h-9/)
    await act(async () => button.click())
    assert.equal(creates, 1)
    const stats = view.container.querySelector('[aria-label="Task statistics"]')!
    assert.equal(stats.querySelectorAll('svg').length, 0)
    assert.match(stats.textContent ?? '', /Scheduled2tasksArmed1armed/)
    assert.match(stats.textContent ?? '', /Failures1last run/)
  } finally { await view.unmount() }
})
