import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import type { DepotProviderOption, FederatedArtifact } from '@/lib/api/depot-client'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'

const providers = [{ id: 'source-a', name: 'First source', enabled: true }, { id: 'source-b', name: 'Disabled source', enabled: false }] as DepotProviderOption[]

test('source controls preserve exact provider IDs and applied filters can be cleared', async () => {
  const window = installTestDom()
  for (const name of ['Event', 'NodeFilter', 'HTMLInputElement'] as const) Object.defineProperty(globalThis, name, { value: window[name], configurable: true })
  const { DiscoverSearchControls } = await import('./discover-search-controls')
  const changes: Array<[string, string]> = []
  let filtersOpen = false
  const view = await renderClient(<DiscoverSearchControls query="" onQuery={() => {}} providers={providers} artifacts={[]} kind="skill" selectedProvider="source-a" onFilter={(field, value) => changes.push([field, value])} filtersOpen={false} onFiltersOpenChange={value => { filtersOpen = value }} totalCount={26} onClearAll={() => {}} />)
  try {
    const enabled = view.container.querySelector<HTMLButtonElement>('[aria-label="Filter to First source"]')!
    assert.equal(enabled.getAttribute('aria-pressed'), 'true')
    assert.equal(view.container.querySelector<HTMLButtonElement>('[aria-label="Filter to Disabled source"]')?.disabled, true)
    await act(async () => enabled.click())
    assert.deepEqual(changes, [['provider', 'all']])
    assert.equal(filtersOpen, true)
    await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Remove Kind: skill filter"]')!.click())
    assert.deepEqual(changes, [['provider', 'all'], ['kind', 'all']])
    await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Kind and source filters"]')!.click())
    assert.equal(filtersOpen, true)
    assert.match(view.container.querySelector<HTMLInputElement>('[aria-label="Search artifacts"]')?.placeholder ?? '', /Search 26 artifacts/)
  } finally { await view.unmount(); await window.happyDOM.close() }
})

test('provider brands require an unambiguous returned source origin', async () => {
  const { discoveryProviderOrigins } = await import('./discover-search-controls')
  const observed = discoveryProviderOrigins([
    { providerId: 'aggregate', sourceOrigin: 'claude' }, { providerId: 'aggregate', sourceOrigin: 'github' },
    { providerId: 'source', sourceOrigin: 'gemini' }, { providerId: 'source', sourceOrigin: 'gemini' },
    { providerId: 'named-github' },
  ] as FederatedArtifact[])
  assert.deepEqual([...observed], [['source', 'gemini']])
})

test('visibility is offered after search and remains a loaded-result filter', async () => {
  const window = installTestDom()
  for (const name of ['Event', 'NodeFilter', 'HTMLInputElement'] as const) Object.defineProperty(globalThis, name, { value: window[name], configurable: true })
  const { DiscoverFilterPanel } = await import('./discover-search-controls')
  let visibility = 'all'
  const view = await renderClient(<DiscoverFilterPanel open providers={providers} artifacts={[]} kind="all" selectedProvider="all" onFilter={() => {}} onVisibility={value => { visibility = value }} />)
  try {
    const team = Array.from(document.querySelectorAll<HTMLButtonElement>('button')).find(button => button.textContent === 'Team')!
    assert.ok(team)
    await act(async () => team.click())
    assert.equal(visibility, 'team')
    assert.match(document.body.textContent ?? '', /reported by each source/)
    assert.match(document.body.textContent ?? '', /All Sources/)
  } finally { await view.unmount(); await window.happyDOM.close() }
})
