import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { ContainerWizardFooter } from './container-wizard-footer'

test('wizard footer keeps navigation labels visible and dispatches step changes', async () => {
  installTestDom()
  const steps: number[] = []
  const view = await renderClient(<ContainerWizardFooter step={2} summary="Step 3 of 7" onStep={step => steps.push(step)}/>)
  try {
    const buttons = Array.from(view.container.querySelectorAll('button'))
    assert.deepEqual(buttons.map(button => button.textContent), ['Back', 'Next'])
    assert.ok(buttons.every(button => button.dataset.visibleLabel === '1'))
    await act(async () => { buttons[0].click(); buttons[1].click() })
    assert.deepEqual(steps, [1, 3])
  } finally { await view.unmount() }
})
