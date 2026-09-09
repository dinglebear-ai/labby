import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { AgentSessionsTable } from './agent-sessions-table'

test('compact session rows preserve sorting, keyboard opening, and visible status labels', async () => {
  installTestDom()
  const row = ['Running', 'Example session', 'team', 'linux', 'Claude Code', '2m']
  const selected: string[][] = [], sorted: number[] = []
  const view = await renderClient(<AgentSessionsTable rows={[row]} onSelect={item => selected.push(item)} onSort={column => sorted.push(column)} renderMark={name => <span>{name} mark</span>} />)
  try {
    assert.match(view.container.querySelector('tbody td')?.textContent ?? '', /Running/)
    assert.ok(view.container.querySelector('td:nth-child(4)')?.classList.contains('max-[1180px]:hidden'))
    assert.ok(view.container.querySelector('td:nth-child(5)')?.classList.contains('max-[1400px]:hidden'))
    await act(async () => view.container.querySelector<HTMLButtonElement>('th:nth-child(2) button')!.click())
    assert.deepEqual(sorted, [1])
    await act(async () => view.container.querySelector('tbody tr')!.dispatchEvent(new window.KeyboardEvent('keydown', { key: ' ', bubbles: true })))
    assert.deepEqual(selected, [row])
  } finally { await view.unmount() }
})
