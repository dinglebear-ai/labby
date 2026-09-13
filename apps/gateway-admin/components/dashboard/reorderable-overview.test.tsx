import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'
import { ReorderableOverview, nearestOverviewDrop, normalizeOverviewOrder, normalizeOverviewWidths, normalizeOverviewLayout, placeOverviewCard } from './reorderable-overview'

installTestDom()
const key = 'labby:overview-layout:v2'
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
    assert.ok(toggle().closest('[data-overview-card]')!.classList.contains('min-[700px]:col-span-2'))
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
    assert.deepEqual(JSON.parse(window.localStorage.getItem(key)!).order, ['r', 'b', 'a', 's'])
  } finally { await view.unmount() }
})

test('keyboard reorder restores focus to the moved card after same-lane and cross-lane moves', async () => {
  window.localStorage.clear()
  const view = await renderClient(<ReorderableOverview cards={cards}/>)
  try {
    const handle = () => document.querySelector<HTMLButtonElement>('[aria-label="Drag a"]')!
    handle().focus()
    await act(async () => handle().dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true })))
    assert.equal(document.activeElement?.getAttribute('aria-label'), 'Drag a')
    assert.deepEqual(lane('telemetry'), ['b', 'a'])

    const crossLane = document.querySelector<HTMLButtonElement>('[aria-label="Move a to insights column"]')!
    crossLane.focus()
    await act(async () => crossLane.click())
    assert.equal(document.activeElement?.getAttribute('aria-label'), 'Drag a')
    assert.deepEqual(lane('insights'), ['r', 's', 'a'])
  } finally { await view.unmount(); window.localStorage.clear() }
})

test('pointer capture failure clears pending state so a later drag can start', async () => {
  window.localStorage.clear()
  const view = await renderClient(<ReorderableOverview cards={cards}/>)
  try {
    const handle = document.querySelector<HTMLButtonElement>('[aria-label="Drag a"]')!
    let attempts = 0
    const releases: number[] = []
    Object.defineProperties(handle, {
      setPointerCapture: { configurable: true, value: () => { attempts += 1; if (attempts === 1) throw new DOMException('Inactive pointer', 'NotFoundError') } },
      hasPointerCapture: { configurable: true, value: () => attempts > 1 },
      releasePointerCapture: { configurable: true, value: (pointerId: number) => releases.push(pointerId) },
    })

    await act(async () => handle.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, button: 0, pointerId: 1, clientX: 10, clientY: 10 })))
    assert.equal(attempts, 1)
    assert.equal(document.querySelector('[role="status"]')?.textContent, 'Could not start pointer drag for a. Use the move controls instead.')

    await act(async () => handle.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, button: 0, pointerId: 2, clientX: 10, clientY: 10 })))
    assert.equal(attempts, 2)
    await act(async () => handle.dispatchEvent(new PointerEvent('pointercancel', { bubbles: true, pointerId: 2 })))
    assert.deepEqual(releases, [2])
  } finally { await view.unmount(); window.localStorage.clear() }
})

test('Escape and unmount release an owned pointer capture', async () => {
  window.localStorage.clear()
  const view = await renderClient(<ReorderableOverview cards={cards}/>)
  const handle = document.querySelector<HTMLButtonElement>('[aria-label="Drag a"]')!
  const captured = new Set<number>()
  const releases: number[] = []
  Object.defineProperties(handle, {
    setPointerCapture: { configurable: true, value: (pointerId: number) => captured.add(pointerId) },
    hasPointerCapture: { configurable: true, value: (pointerId: number) => captured.has(pointerId) },
    releasePointerCapture: { configurable: true, value: (pointerId: number) => { captured.delete(pointerId); releases.push(pointerId) } },
  })

  await act(async () => handle.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, button: 0, pointerId: 3, clientX: 10, clientY: 10 })))
  await act(async () => handle.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })))
  assert.deepEqual(releases, [3])
  assert.equal(document.querySelector('[role="status"]')?.textContent, 'Card move cancelled.')

  await act(async () => handle.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, button: 0, pointerId: 4, clientX: 10, clientY: 10 })))
  await view.unmount()
  assert.deepEqual(releases, [3, 4])
  window.localStorage.clear()
})

test('cross-column moves preserve width and survive remount without duplicates', async () => {
  window.localStorage.clear()
  window.localStorage.setItem('labby:overview-card-order:v1', JSON.stringify(['b', 'b', null, 'unknown']))
  let view = await renderClient(<ReorderableOverview cards={cards}/>)
  try {
    assert.deepEqual(lane('telemetry'), ['b', 'a'])
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Move b to insights column"]')!.click())
    assert.deepEqual(lane('telemetry'), ['a'])
    assert.deepEqual(lane('insights'), ['r', 's', 'b'])
    assert.equal(document.querySelector('[aria-label="Toggle b full width"]'), null)
    await view.unmount()
    view = await renderClient(<ReorderableOverview cards={cards}/>)
    assert.deepEqual(lane('insights'), ['r', 's', 'b'])
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Move b to telemetry column"]')!.click())
    assert.deepEqual(lane('telemetry'), ['a', 'b'])
    assert.equal(document.querySelectorAll('[data-card="b"]').length, 1)
    assert.equal(document.querySelector('[data-overview-card][draggable]'), null)
  } finally { await view.unmount(); window.localStorage.clear() }
})

test('drop edges place adjacent and nonadjacent cards in both directions', () => {
  const initial = normalizeOverviewLayout(null, ['a', 'b', 'c', 'd'])
  assert.deepEqual(placeOverviewCard(initial, 'a', { id: 'b', lane: 'telemetry', after: true }).order, ['b', 'a', 'c', 'd'])
  assert.deepEqual(placeOverviewCard(initial, 'a', { id: 'd', lane: 'telemetry', after: true }).order, ['b', 'c', 'd', 'a'])
  assert.deepEqual(placeOverviewCard(initial, 'd', { id: 'b', lane: 'telemetry', after: false }).order, ['a', 'd', 'b', 'c'])
  assert.equal(placeOverviewCard(initial, 'b', { id: 'b', lane: 'insights', after: true }), initial)
  assert.equal(placeOverviewCard(initial, 'missing', { id: 'b', lane: 'insights', after: true }), initial)
  const crossed = placeOverviewCard(initial, 'b', { id: null, lane: 'insights', after: true })
  assert.deepEqual(crossed.order, ['a', 'c', 'd', 'b'])
  assert.equal(crossed.lanes.b, 'insights')
})

test('lane gaps resolve to the nearest visible insertion while empty and below-end space append', () => {
  const cards = [
    { id: 'first', left: 0, right: 100, top: 0, bottom: 100 },
    { id: 'second', left: 0, right: 100, top: 112, bottom: 212 },
  ]
  assert.deepEqual(nearestOverviewDrop(cards, 50, 108), { id: 'second', after: false })
  assert.deepEqual(nearestOverviewDrop(cards, 50, 104), { id: 'first', after: true })
  assert.deepEqual(nearestOverviewDrop(cards, 50, 220), { id: null, after: true })
  assert.deepEqual(nearestOverviewDrop([], 50, 50), { id: null, after: true })
})

test('saved layout rejects unknown columns and accepts only valid card identities', () => {
  assert.deepEqual(normalizeOverviewLayout({ order: ['b', 'b'], widths: { b: true, extra: true }, lanes: { b: 'insights', a: 'bogus', extra: 'telemetry' } }, ['a', 'b']), { order: ['b', 'a'], widths: { b: true }, lanes: { b: 'insights' } })
})
