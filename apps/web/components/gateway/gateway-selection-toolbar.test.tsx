import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'
import type { Gateway } from '@/lib/types/gateway'

installTestDom()
test('confirmed snapshot ignores new selection and unmount stops remaining requests', async () => {
  const { GatewaySelectionToolbar } = await import('./gateway-selection-toolbar')
  const gateways = ['a', 'b', 'c'].map(id => ({ id, name: id, enabled: true, transport: 'http' }) as Gateway)
  const sent: string[] = []
  let finish!: (value: { ok: true }) => void
  const callback = async (gateway: Gateway) => { sent.push(gateway.id); return new Promise<{ ok: true }>(resolve => { finish = resolve }) }
  const props = { gateways, onClear: () => {}, onBatchSetEnabled: callback }
  const view = await renderClient(<GatewaySelectionToolbar {...props} selectedIds={['a', 'b']}/>)
  await act(async () => [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === 'disable')!.click())
  await view.rerender(<GatewaySelectionToolbar {...props} selectedIds={['c']}/>)
  assert.match(document.body.textContent ?? '', /a \(a\), b \(b\)/)
  await act(async () => [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === 'Confirm batch')!.click())
  assert.deepEqual(sent, ['a'])
  await view.unmount()
  await act(async () => finish({ ok: true }))
  assert.deepEqual(sent, ['a'])
})

test('batch toolbar confirms named snapshot and locks duplicate submissions', async () => {
  const { GatewaySelectionToolbar } = await import('./gateway-selection-toolbar')
  const gateway = { id: 'a', name: 'Confirmed server', enabled: true, transport: 'http' } as Gateway
  let calls = 0
  let finish!: (value: { ok: true }) => void
  const view = await renderClient(<GatewaySelectionToolbar gateways={[gateway]} selectedIds={['a']} onClear={() => {}} onBatchSetEnabled={async () => { calls++; return new Promise(resolve => { finish = resolve }) }}/>)
  try {
    const disable = [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === 'disable')!
    await act(async () => disable.click())
    assert.equal(calls, 0)
    assert.match(document.body.textContent ?? '', /Confirmed server \(a\)/)
    const confirm = [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === 'Confirm batch')!
    await act(async () => { confirm.click(); confirm.click() })
    assert.equal(calls, 1)
    assert.equal(disable.disabled, true)
    await act(async () => finish({ ok: true }))
    assert.match(document.body.textContent ?? '', /Confirmed server: completed/)
  } finally { await view.unmount() }
})
