import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { ExternalClientSetup } from './ExternalClientSetup'

test('selected clients reveal local commands without claiming a connection', async () => {
  installTestDom()
  const view = await renderClient(<ExternalClientSetup mcpEndpoint="https://lab.example/mcp" />)
  try {
    assert.equal(view.container.querySelectorAll('code').length, 0)
    const checkbox = view.container.querySelector<HTMLButtonElement>('#client-codex')!
    await act(async () => checkbox.click())
    assert.deepEqual(Array.from(view.container.querySelectorAll('code')).map(node => node.textContent), [
      'labby setup clients connect --clients codex',
      "labby setup clients connect --clients codex --connection oauth --gateway-url 'https://lab.example/mcp'",
    ])
    assert.match(view.container.textContent!, /browser cannot edit their local configuration/)
    assert.match(view.container.textContent!, /does not mark applications connected/)
    assert.match(view.container.textContent!, /Each application completes its own OAuth login as the same user/)
    await act(async () => checkbox.click())
    assert.equal(view.container.querySelectorAll('code').length, 0)
  } finally { await view.unmount() }
})
