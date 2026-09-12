import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'

import { gatewayApi } from '../api/gateway-client'
import { installTestDom, renderClient } from '../testing/dom-test-utils.tsx'
import { GATEWAYS_KEY, gatewaysRequestKey, gatewaysRuntimeRequestKey, useGatewaySnapshots } from './use-gateways'

test('gateway loading can be disabled for closed demand-driven dialogs', () => {
  assert.equal(gatewaysRequestKey(false), null)
  assert.equal(gatewaysRequestKey(true), GATEWAYS_KEY)
})

test('configuration-only gateway loading never hydrates fleet runtime state', () => {
  const gateways = [{ id: 'alpha' }] as Parameters<typeof gatewaysRuntimeRequestKey>[2]
  assert.equal(gatewaysRuntimeRequestKey(false, true, gateways), null)
  assert.equal(gatewaysRuntimeRequestKey(true, false, gateways), null)
  assert.deepEqual(gatewaysRuntimeRequestKey(true, true, gateways), ['/gateways/runtime', '[{"id":"alpha"}]'])
})

test('gateway snapshots stay idle until their consumer is enabled', async () => {
  installTestDom()
  const originalList = gatewayApi.list
  let listCalls = 0
  gatewayApi.list = async () => { listCalls += 1; return [] }
  function Harness({ enabled }: { enabled: boolean }) {
    const result = useGatewaySnapshots(enabled)
    return React.createElement('span', null, result.isLoading ? 'loading' : 'ready')
  }

  const view = await renderClient(React.createElement(Harness, { enabled: false }))
  try {
    await act(async () => {})
    assert.equal(listCalls, 0)
    await view.rerender(React.createElement(Harness, { enabled: true }))
    for (let attempt = 0; attempt < 50 && listCalls === 0; attempt += 1) {
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)) })
    }
    assert.equal(listCalls, 1)
  } finally {
    gatewayApi.list = originalList
    await view.unmount()
  }
})


test('same-ID configuration edits create a fresh runtime cache entry', () => {
  const oldRows = [{ id: 'alpha', enabled: true, name: 'old gateway' }] as Parameters<typeof gatewaysRuntimeRequestKey>[2]
  const newRows = [{ id: 'alpha', enabled: false, name: 'new gateway' }] as Parameters<typeof gatewaysRuntimeRequestKey>[2]
  assert.notDeepEqual(gatewaysRuntimeRequestKey(true, true, oldRows), gatewaysRuntimeRequestKey(true, true, newRows))
})

test('same-ID edits replace hydrated rows even when catalog polling is inactive', async () => {
  const { SWRConfig, useSWRConfig } = await import('swr')
  const { useGateways } = await import('./use-gateways')
  const { mockGateways } = await import('../api/mock-data')
  installTestDom()
  const originalList = gatewayApi.list
  const originalHydrate = gatewayApi.hydrateRuntime
  const originalRefresh = gatewayApi.refreshStatus
  const first = { ...mockGateways[0], id: 'configuration-regression', name: 'Before edit' }
  const second = { ...first, name: 'After edit', enabled: false }
  let update: ReturnType<typeof useSWRConfig>['mutate']
  const hydrated: string[] = []
  gatewayApi.list = async () => [first]
  gatewayApi.hydrateRuntime = async rows => { hydrated.push(rows[0]?.name); return rows }
  // Catalog warming has stopped; configuration changes must independently
  // invalidate hydrated rows, without relying on its refresh interval.
  gatewayApi.refreshStatus = async () => { throw new Error('warming unavailable') }
  function Harness() {
    update = useSWRConfig().mutate
    const rows = useGateways()
    return React.createElement('span', null, rows.data?.[0]?.name ?? 'loading')
  }
  const view = await renderClient(React.createElement(SWRConfig, { value: { provider: () => new Map(), shouldRetryOnError: false } }, React.createElement(Harness)))
  const waitForName = async (name: string) => {
    for (let attempt = 0; attempt < 50 && view.container.textContent !== name; attempt++) {
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    assert.equal(view.container.textContent, name)
  }
  try {
    await waitForName('Before edit')
    await act(async () => { await update(GATEWAYS_KEY, [second], false) })
    await waitForName('After edit')
    assert.ok(hydrated.includes('After edit'))
  } finally {
    await view.unmount()
    gatewayApi.list = originalList
    gatewayApi.hydrateRuntime = originalHydrate
    gatewayApi.refreshStatus = originalRefresh
  }
})
