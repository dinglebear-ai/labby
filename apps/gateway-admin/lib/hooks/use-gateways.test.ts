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
  assert.deepEqual(gatewaysRuntimeRequestKey(true, true, gateways), ['/gateways/runtime', 'alpha:1'])
})

test('the runtime key changes when a gateway is enabled or disabled', () => {
  // Keying only on the id list meant a disable/enable was cache-identical, so
  // the hydrated runtime view kept serving the pre-toggle snapshot until a
  // full reload. bead lab-gz4gk.
  const enabled = [{ id: 'alpha' }] as Parameters<typeof gatewaysRuntimeRequestKey>[2]
  const disabled = [{ id: 'alpha', enabled: false }] as Parameters<typeof gatewaysRuntimeRequestKey>[2]

  assert.deepEqual(gatewaysRuntimeRequestKey(true, true, enabled), ['/gateways/runtime', 'alpha:1'])
  assert.deepEqual(gatewaysRuntimeRequestKey(true, true, disabled), ['/gateways/runtime', 'alpha:0'])
  assert.notDeepEqual(
    gatewaysRuntimeRequestKey(true, true, enabled),
    gatewaysRuntimeRequestKey(true, true, disabled),
  )
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
