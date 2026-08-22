import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'

import type { GatewayLoadoutInput } from '@/lib/types/gateway'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils.tsx'

const existing = {
  name: 'operations',
  description: null,
  upstreams: ['alpha'],
  services: [],
  expose_code_mode: false,
  expose_tools: true,
  expose_resources: true,
  expose_prompts: true,
  expose_skills: true,
}

function saveButton() {
  return [...document.querySelectorAll('button')].find((button) => button.textContent?.includes('Save changes'))
}

async function waitForSaveButton() {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    const button = saveButton()
    if (button) return button
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)) })
  }
  return undefined
}

test('loadout dialog preserves existing upstreams when gateway options are unavailable', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'getComputedStyle', { value: window.getComputedStyle.bind(window), configurable: true })
  Object.defineProperty(globalThis, 'MutationObserver', { value: window.MutationObserver, configurable: true })
  Object.defineProperty(globalThis, 'Event', { value: window.Event, configurable: true })
  Object.defineProperty(globalThis, 'CustomEvent', { value: window.CustomEvent, configurable: true })
  Object.defineProperty(globalThis, 'NodeFilter', { value: window.NodeFilter, configurable: true })
  Object.defineProperty(globalThis, 'HTMLElement', { value: window.HTMLElement, configurable: true })
  Object.defineProperty(globalThis, 'HTMLInputElement', { value: window.HTMLInputElement, configurable: true })
  const { LoadoutFormDialog, loadoutSaveEnabled } = await import('./loadout-form-dialog.tsx')
  assert.equal(loadoutSaveEnabled(false, false, 'Gateway options failed', 'services', 1, false), true)
  let saved: GatewayLoadoutInput | undefined
  const base = {
    open: true,
    loadout: existing,
    gatewayOptions: [],
    serviceOptions: [],
    onOpenChange: () => {},
    onSave: async (_original: string | null, draft: GatewayLoadoutInput) => { saved = draft },
  }
  const view = await renderClient(<LoadoutFormDialog {...base} gatewayOptionsLoading />)
  try {
    const loadingSave = await waitForSaveButton()
    assert.ok(loadingSave, document.body.innerHTML)
    assert.equal(loadingSave.disabled, false)
    assert.match(document.body.textContent ?? '', /Loading gateway options/)

    await view.rerender(<LoadoutFormDialog {...base} gatewayOptionsError="Gateway options failed" />)
    assert.equal(saveButton()?.disabled, false)
    assert.match(document.body.textContent ?? '', /Gateway options failed/)

    await view.rerender(<LoadoutFormDialog {...base} gatewayOptions={[{ value: 'alpha', label: 'alpha' }]} />)
    const save = saveButton()
    assert.ok(save)
    assert.equal(save.disabled, false)
    await act(async () => { save.click() })
    assert.deepEqual(saved?.upstreams, ['alpha'])
  } finally {
    await view.unmount()
  }
})
