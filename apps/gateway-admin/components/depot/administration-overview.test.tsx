import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { AdministrationOverview } from './administration-overview'

test('overview cards retain all three workspace actions in the compact layout', async () => {
  const window = installTestDom()
  const selected: string[] = []
  const view = await renderClient(<AdministrationOverview onOpen={workspace => selected.push(workspace)} />)
  try {
    const buttons = view.container.querySelectorAll<HTMLButtonElement>('button')
    assert.equal(buttons.length, 3)
    assert.equal(view.container.querySelectorAll('h2').length, 3)
    assert.equal(view.container.querySelectorAll('svg').length, 0)
    for (const button of buttons) {
      assert.match(button.className, /h-\[30px\]/)
      await act(async () => button.dispatchEvent(new window.MouseEvent('click', { bubbles: true })))
    }
    assert.deepEqual(selected, ['catalog', 'access', 'operations'])
  } finally { await view.unmount() }
})
