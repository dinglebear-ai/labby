import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'

test('long press enters Discover selection mode and the follow-up click selects instead of navigating', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'self', { value: window, configurable: true })
  for (const name of ['Event','MouseEvent','HTMLAnchorElement'] as const) Object.defineProperty(globalThis, name, { value: window[name], configurable: true })
  const { DiscoverArtifactCard } = await import('./discover-artifact-card')
  let entered = 0, toggled = 0
  const view = await renderClient(<DiscoverArtifactCard artifact={{ providerId:'p', artifactId:'repo-triage', title:'repo-triage', kind:'skill' }} compact={false} selected={false} href="/depot?artifact=repo-triage" onEnterSelectionMode={() => { entered += 1 }} onToggleSelected={() => { toggled += 1 }} />)
  try {
    const link = view.container.querySelector<HTMLAnchorElement>('a[data-artifact-key]')!
    await act(async () => { link.dispatchEvent(new MouseEvent('mousedown', { bubbles:true, button:0 })); await new Promise(resolve => setTimeout(resolve, 440)); link.dispatchEvent(new MouseEvent('mouseup', { bubbles:true, button:0 })) })
    assert.equal(entered, 1)
    const click = new MouseEvent('click', { bubbles:true, cancelable:true })
    await act(async () => { link.dispatchEvent(click) })
    assert.equal(click.defaultPrevented, true)
    assert.equal(toggled, 1)
  } finally { await view.unmount(); await window.happyDOM.close() }
})

test('selection mode renders the mock checkbox and selected state', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'self', { value: window, configurable: true })
  const { DiscoverArtifactCard } = await import('./discover-artifact-card')
  const view = await renderClient(<DiscoverArtifactCard artifact={{ providerId:'p', artifactId:'bundle', title:'Bundle', kind:'loadout' }} compact={false} selected={false} href="/depot" selectionMode selectedForBulk onToggleSelected={() => {}} />)
  try {
    const checkbox = view.container.querySelector<HTMLButtonElement>('button[aria-label="Select Bundle"]')
    assert.ok(checkbox)
    assert.equal(checkbox.getAttribute('aria-pressed'), 'true')
  } finally { await view.unmount(); await window.happyDOM.close() }
})
