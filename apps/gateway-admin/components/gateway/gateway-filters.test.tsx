import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { GatewayFilters } from './gateway-filters'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'

installTestDom()

test('closed filter controls retain removable chips with exact filter identities', async () => {
  const calls: string[] = []
  const view = await renderClient(<GatewayFilters mode="tools" search="needle" gatewayFilters={{ status: [], source: [], transport: [] }} toolFilters={{ search: 'needle', gatewayIds: ['missing-label'], exposure: 'hidden', source: ['custom'], transport: ['http'] }} gatewayOptions={[]} mobileSheetOpen={false} onMobileSheetOpenChange={() => {}} onSearchChange={value => calls.push(`search:${value}`)} onGatewayFilterToggle={() => {}} onToolFilterToggle={(group, value) => calls.push(`${group}:${value}`)} onExposureChange={value => calls.push(`exposure:${value}`)} onClearFilters={() => calls.push('all')}/>)
  try {
    const strip = document.querySelector('[aria-label="Active filters"]')!
    assert.ok(strip)
    for (const label of ['Search: needle', 'missing-label', 'Hidden only', 'Custom', 'HTTP']) {
      await act(async () => strip.querySelector<HTMLButtonElement>(`[aria-label="Remove ${label} filter"]`)!.click())
    }
    assert.deepEqual(calls, ['search:', 'gatewayIds:missing-label', 'exposure:all', 'source:custom', 'transport:http'])
  } finally { await view.unmount() }
})

test('gateway filters keep facets behind the search bar filter toggle', () => {
  const markup = renderToStaticMarkup(
    <GatewayFilters
      mode="gateways"
      search="gateway_beta"
      gatewayFilters={{ status: ['configured'], source: ['lab'], transport: ['stdio'] }}
      toolFilters={{ search: '', gatewayIds: [], exposure: 'all', source: [], transport: [] }}
      gatewayOptions={[]}
      mobileSheetOpen={true}
      onMobileSheetOpenChange={() => {}}
      onSearchChange={() => {}}
      onGatewayFilterToggle={() => {}}
      onToolFilterToggle={() => {}}
      onExposureChange={() => {}}
      onClearFilters={() => {}}
    />,
  )

  assert.match(markup, /bg-aurora-panel-medium/)
  assert.match(markup, /bg-aurora-control-surface/)
  assert.match(markup, /data-mobile-search="gateways"/)
  assert.match(markup, /Search servers/)
  assert.match(markup, /Clear/i)
  assert.match(markup, /aria-label="Open filters"/)
  assert.match(markup, /aria-label="Toggle filters"/)
  assert.match(markup, /aria-pressed="true"/)
  assert.match(markup, /Configured/)
  assert.doesNotMatch(markup, /role="combobox"/)
})

test('tools filters render exposure segmented control and gateway facets', () => {
  const markup = renderToStaticMarkup(
    <GatewayFilters
      mode="tools"
      search="uni"
      gatewayFilters={{ status: [], source: [], transport: [] }}
      toolFilters={{ search: '', gatewayIds: ['gw_1'], exposure: 'exposed', source: ['lab'], transport: ['stdio'] }}
      gatewayOptions={[{ value: 'gw_1', label: 'Lab Core' }]}
      mobileSheetOpen={true}
      onMobileSheetOpenChange={() => {}}
      onSearchChange={() => {}}
      onGatewayFilterToggle={() => {}}
      onToolFilterToggle={() => {}}
      onExposureChange={() => {}}
      onClearFilters={() => {}}
    />,
  )

  assert.match(markup, /Exposed only/)
  assert.match(markup, /Lab Core/)
  assert.match(markup, /data-mobile-search="tools"/)
  assert.match(markup, /aria-label="Open filters"/)
  assert.match(markup, /Search tools, descriptions, or servers/)
})
