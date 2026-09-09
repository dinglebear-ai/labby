import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { ContainerOption } from './container-option'

test('container option exposes its selected state and keeps the full name accessible', async () => {
  installTestDom()
  let toggles = 0
  const view = await renderClient(<ContainerOption name="Ubuntu 24.04" detail="base image" selected onToggle={() => toggles++}/>)
  try {
    const button = view.container.querySelector('button')!
    assert.equal(button.getAttribute('aria-pressed'), 'true')
    assert.match(button.textContent ?? '', /Ubuntu 24.04base image/)
    assert.match(button.innerHTML, /size-\[30px\]/)
    assert.match(button.innerHTML, /size-\[17px\]/)
    await act(async () => button.click())
    assert.equal(toggles, 1)
  } finally { await view.unmount() }
})
