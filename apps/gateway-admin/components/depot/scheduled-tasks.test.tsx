import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { ScheduledTasks } from './scheduled-tasks'

test('compact task table preserves filtering, selection, and arm controls', async () => {
  installTestDom()
  const rows = [['Armed', 'Audit', 'Daily', 'team', 'tomorrow', 'Inspect metadata', 'passed'], ['Paused', 'Review', 'Weekly', 'team', 'paused', 'Review changes', 'pending']]
  const selected: string[][] = [], toggled: string[][] = []
  const view = await renderClient(<ScheduledTasks rows={rows} onSelect={row => selected.push(row)} onToggle={row => toggled.push(row)} />)
  try {
    assert.equal(view.container.querySelectorAll('tbody tr').length, 2)
    for (const [column, breakpoint] of [[4, 1180], [5, 1400]]) {
      for (const selector of [`col:nth-child(${column})`, `th:nth-child(${column})`, `td:nth-child(${column})`]) {
        assert.ok(view.container.querySelector(selector)?.classList.contains(`max-[${breakpoint}px]:hidden`), `${selector} shares the mock breakpoint`)
      }
    }
    const arm = view.container.querySelector<HTMLButtonElement>('[role="switch"]')!
    assert.match(arm.className, /h-\[18px\]/)
    assert.equal(arm.getAttribute('aria-checked'), 'true')
    await act(async () => arm.click())
    assert.deepEqual(toggled, [rows[0]])
    const task = view.container.querySelector<HTMLButtonElement>('tbody tr td:nth-child(2) button')!
    await act(async () => task.click())
    assert.equal(task.getAttribute('aria-expanded'), 'true')
    assert.deepEqual(selected, [], 'opening inline details does not open the edit dialog')
    assert.match(view.container.textContent ?? '', /Run history is unavailable/)
    const edit = [...view.container.querySelectorAll('button')].find(button => button.textContent === 'Edit task')!
    await act(async () => edit.click())
    assert.deepEqual(selected, [rows[0]])
    const paused = [...view.container.querySelectorAll('button')].find(button => button.textContent === 'Paused')!
    await act(async () => paused.click())
    assert.equal(view.container.querySelectorAll('tbody tr').length, 1)
    assert.match(view.container.querySelector('tbody')?.textContent ?? '', /Review/)
    assert.ok(view.container.querySelector('a[href="/logs"]'))
  } finally { await view.unmount() }
})

test('expanded details span only visible columns at both mock breakpoints', async () => {
  installTestDom()
  const original = window.matchMedia
  try {
    for (const [width, columns] of [[1120, 5], [1280, 6], [1500, 7]]) {
      window.matchMedia = query => ({ matches: width < Number(query.match(/\d+/)?.[0]), media: query, addEventListener() {}, removeEventListener() {} } as unknown as MediaQueryList)
      const view = await renderClient(<ScheduledTasks rows={[['Armed', 'Audit', 'Daily', 'team', 'tomorrow', 'Inspect', 'passed']]} onSelect={() => {}} onToggle={() => {}} />)
      try {
        await act(async () => view.container.querySelector<HTMLButtonElement>('td:nth-child(2) button')!.click())
        assert.equal(view.container.querySelector('td[colspan]')?.getAttribute('colspan'), String(columns), `${width}px viewport`)
      } finally { await view.unmount() }
    }
  } finally { window.matchMedia = original }
})
