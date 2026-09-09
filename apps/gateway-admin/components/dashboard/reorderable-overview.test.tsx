import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'
import { ReorderableOverview, normalizeOverviewOrder, normalizeOverviewWidths } from './reorderable-overview'

const dom = installTestDom()
const key = 'labby:overview-card-order:v1'
const cards = [
  { id: 'a', content: <span data-card="a">A</span> },
  { id: 'r', rail: true, content: <span data-card="r">R</span> },
  { id: 'b', content: <span data-card="b">B</span> },
  { id: 's', rail: true, content: <span data-card="s">S</span> },
]
const lane = (name: string) => [...document.querySelectorAll(`[data-overview-lane="${name}"] [data-card]`)].map(node => node.getAttribute('data-card'))

test('width preferences accept only known boolean entries', () => {
  assert.deepEqual(normalizeOverviewWidths({ a: false, b: true, unknown: true, r: 'true' }, ['a', 'b', 'r']), { a: false, b: true })
  for (const invalid of [null, [], 'wide', 1]) assert.deepEqual(normalizeOverviewWidths(invalid, ['a']), {})
})

test('width control changes only the selected main card and survives a remount', async () => {
  window.localStorage.clear()
  let view = await renderClient(<ReorderableOverview cards={cards}/>)
  try {
    assert.equal(document.querySelector('[aria-label="Toggle r full width"]'), null)
    const toggle = () => document.querySelector<HTMLButtonElement>('[aria-label="Toggle a full width"]')!
    assert.equal(toggle().getAttribute('aria-pressed'), 'false')
    await act(async () => toggle().click())
    assert.equal(toggle().getAttribute('aria-pressed'), 'true')
    assert.ok(toggle().closest('[draggable]')!.classList.contains('min-[700px]:col-span-2'))
    assert.deepEqual(lane('telemetry'), ['a', 'b'])
    await view.unmount()
    view = await renderClient(<ReorderableOverview cards={cards}/>)
    assert.equal(toggle().getAttribute('aria-pressed'), 'true')
    await act(async () => toggle().click())
    assert.equal(toggle().getAttribute('aria-pressed'), 'false')
  } finally { await view.unmount(); window.localStorage.clear() }
})

test('saved order rejects invalid duplicate and unknown identities and appends missing cards', () => {
  assert.deepEqual(normalizeOverviewOrder(['b', 'b', 7, null, 'unknown'], ['a', 'r', 'b', 's']), ['b', 'a', 'r', 's'])
  for (const invalid of [null, {}, 'b', 3]) assert.deepEqual(normalizeOverviewOrder(invalid, ['a', 'b']), ['a', 'b'])
})

test('keyboard-accessible move buttons stay within each lane and persist the order', async () => {
  window.localStorage.clear()
  const view = await renderClient(<ReorderableOverview cards={cards}/>)
  try {
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Move a later"]')!.click())
    assert.deepEqual(lane('telemetry'), ['b', 'a'])
    assert.deepEqual(lane('insights'), ['r', 's'])
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Move a later"]')!.click())
    assert.deepEqual(lane('telemetry'), ['b', 'a'])
    assert.deepEqual(JSON.parse(window.localStorage.getItem(key)!), ['b', 'r', 'a', 's'])
  } finally { await view.unmount() }
})

test('cross-lane drag is rejected and dirty persisted order is normalized in the DOM', async () => {
  window.localStorage.setItem(key, JSON.stringify(['b', 'b', null, 'unknown']))
  const view = await renderClient(<ReorderableOverview cards={cards}/>)
  try {
    assert.deepEqual(lane('telemetry'), ['b', 'a'])
    assert.deepEqual(lane('insights'), ['r', 's'])
    const source = document.querySelector('[data-card="b"]')!.closest('[draggable]')!
    const target = document.querySelector('[data-card="r"]')!.closest('[draggable]')!
    await act(async () => { source.dispatchEvent(new dom.Event('dragstart', { bubbles: true })) })
    await act(async () => { target.dispatchEvent(new dom.Event('drop', { bubbles: true })) })
    assert.deepEqual(lane('telemetry'), ['b', 'a'])
    assert.deepEqual(lane('insights'), ['r', 's'])
  } finally { await view.unmount(); window.localStorage.clear() }
})
