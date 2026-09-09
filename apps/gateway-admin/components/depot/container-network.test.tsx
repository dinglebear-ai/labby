import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act, useState } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { ContainerNetworkOptions, DEFAULT_CONTAINER_NETWORK } from './container-network'

test('network controls preserve parent-owned selections across step unmounts', async () => {
  installTestDom()
  function Harness() {
    const [value, onChange] = useState(DEFAULT_CONTAINER_NETWORK)
    const [visible, setVisible] = useState(true)
    return <><button onClick={() => setVisible(!visible)}>Change step</button>{visible && <ContainerNetworkOptions value={value} onChange={onChange}/>}</>
  }
  const view = await renderClient(<Harness/>)
  try {
    const web = () => view.container.querySelector<HTMLButtonElement>('[aria-label="Outbound web access"]')!
    assert.equal(view.container.querySelectorAll('[role="switch"]').length, 4)
    assert.equal(web().getAttribute('aria-checked'), 'true')
    await act(async () => web().click())
    assert.equal(web().getAttribute('aria-checked'), 'false')
    const navigate = view.container.querySelector<HTMLButtonElement>('button')!
    await act(async () => navigate.click())
    await act(async () => navigate.click())
    assert.equal(web().getAttribute('aria-checked'), 'false')
    assert.equal(view.container.querySelector('[aria-label="LAN access"]')?.getAttribute('aria-checked'), 'false')
  } finally { await view.unmount() }
})
