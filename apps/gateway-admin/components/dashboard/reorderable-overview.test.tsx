import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'
import { ReorderableOverview, nearestOverviewDrop, normalizeOverviewOrder, normalizeOverviewWidths, normalizeOverviewLayout, overviewMasonrySpan, placeOverviewCard, planOverviewPacking } from './reorderable-overview'

installTestDom()
const key = 'labby:overview-layout:v2'
const cards = [
  { id: 'a', content: <span data-card="a">A</span> },
  { id: 'r', rail: true, content: <span data-card="r">R</span> },
  { id: 'b', content: <span data-card="b">B</span> },
  { id: 's', rail: true, content: <span data-card="s">S</span> },
]
const lane = (name: string) => [...document.querySelectorAll(`[data-overview-lane="${name}"] [data-card]`)].map(node => node.getAttribute('data-card'))

test('masonry row spans compact unequal cards without invalid measurements', () => {
  assert.equal(overviewMasonrySpan(100, 4, 12), 7)
  assert.equal(overviewMasonrySpan(212, 4, 12), 14)
  assert.equal(overviewMasonrySpan(0), 1)
  assert.equal(overviewMasonrySpan(Number.NaN), 1)
})

test('desktop packing fills the shortest available column before flowing lower', () => {
  assert.deepEqual(planOverviewPacking([
    { id: 'volume', span: 30, wide: true },
    { id: 'targets', span: 70, wide: false },
    { id: 'outcomes', span: 15, wide: false },
    { id: 'clients', span: 20, wide: false },
    { id: 'least', span: 25, wide: false },
  ]), [
    { id: 'volume', column: 1, row: 1, span: 30, columns: 2 },
    { id: 'targets', column: 3, row: 1, span: 70, columns: 1 },
    { id: 'outcomes', column: 1, row: 31, span: 15, columns: 1 },
    { id: 'clients', column: 2, row: 31, span: 20, columns: 1 },
    { id: 'least', column: 1, row: 46, span: 25, columns: 1 },
  ])
})

test('packing uses every desktop column and degrades safely to one column', () => {
  const packed = planOverviewPacking([
    { id: 'a', span: 40, wide: false },
    { id: 'b', span: 10, wide: false },
    { id: 'c', span: 20, wide: false },
    { id: 'd', span: 5, wide: false },
  ])
  assert.deepEqual(packed.map(item => [item.id, item.column, item.row]), [
    ['a', 1, 1], ['b', 2, 1], ['c', 3, 1], ['d', 2, 11],
  ])
  assert.deepEqual(planOverviewPacking([{ id: 'wide', span: 0, wide: true }], 1), [
    { id: 'wide', column: 1, row: 1, span: 1, columns: 1 },
  ])
})

test('width preferences accept only known boolean entries', () => {
  assert.deepEqual(normalizeOverviewWidths({ a: false, b: true, unknown: true, r: 'true' }, ['a', 'b', 'r']), { a: false, b: true })
  for (const invalid of [null, [], 'wide', 1]) assert.deepEqual(normalizeOverviewWidths(invalid, ['a']), {})
})

test('width control changes only the selected main card and survives a remount', async () => {
  window.localStorage.clear()
  let view = await renderClient(<ReorderableOverview cards={cards}/>)
  try {
    assert.equal(document.querySelector('[data-overview-card="r"] [aria-label="Toggle width"]'), null)
    const toggle = () => document.querySelector<HTMLButtonElement>('[data-overview-card="a"] [aria-label="Toggle width"]')!
    assert.equal(toggle().getAttribute('aria-pressed'), 'false')
    await act(async () => toggle().click())
    assert.equal(toggle().getAttribute('aria-pressed'), 'true')
    assert.ok(toggle().closest('[data-overview-card]')!.classList.contains('min-[700px]:col-span-full'))
    assert.deepEqual(lane('telemetry'), ['a', 'b'])
    await view.unmount()
    view = await renderClient(<ReorderableOverview cards={cards}/>)
    assert.equal(toggle().getAttribute('aria-pressed'), 'true')
    await act(async () => toggle().click())
    assert.equal(toggle().getAttribute('aria-pressed'), 'false')
  } finally { await view.unmount(); window.localStorage.clear() }
})

test('desktop classes preserve the mock two-thirds telemetry lane and one-third insight rail', async () => {
  window.localStorage.clear()
  const view = await renderClient(<ReorderableOverview cards={cards}/>)
  try {
    const telemetry = document.querySelector<HTMLElement>('[data-overview-lane="telemetry"]')!
    const insights = document.querySelector<HTMLElement>('[data-overview-lane="insights"]')!
    const columns = document.querySelector<HTMLElement>('[data-overview-columns]')!
    assert.ok(columns.className.includes('min-[1100px]:grid-cols-[minmax(0,2fr)_minmax(260px,1fr)]'))
    assert.ok(telemetry.className.includes('min-[700px]:grid-cols-[repeat(auto-fit,minmax(250px,1fr))]'))
    assert.ok(insights.className.includes('flex-col'))
    assert.ok(!columns.className.includes('grid-cols-3'))
    assert.ok(!telemetry.className.includes('contents'))
  } finally { await view.unmount(); window.localStorage.clear() }
})

test('saved order rejects invalid duplicate and unknown identities and appends missing cards', () => {
  assert.deepEqual(normalizeOverviewOrder(['b', 'b', 7, null, 'unknown'], ['a', 'r', 'b', 's']), ['b', 'a', 'r', 's'])
  for (const invalid of [null, {}, 'b', 3]) assert.deepEqual(normalizeOverviewOrder(invalid, ['a', 'b']), ['a', 'b'])
})

test('Alt+Arrow keyboard reorder stays within each mock-defined lane and persists order', async () => {
  window.localStorage.clear()
  const view = await renderClient(<ReorderableOverview cards={cards}/>)
  try {
    const card = document.querySelector<HTMLElement>('[data-overview-card="a"]')!
    await act(async () => card.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', altKey: true, bubbles: true })))
    assert.deepEqual(lane('telemetry'), ['b', 'a'])
    assert.deepEqual(lane('insights'), ['r', 's'])
    await act(async () => card.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', altKey: true, bubbles: true })))
    assert.deepEqual(lane('telemetry'), ['b', 'a'])
    assert.deepEqual(JSON.parse(window.localStorage.getItem(key)!).order, ['r', 'b', 'a', 's'])
  } finally { await view.unmount() }
})

test('keyboard reorder restores focus to the moved card and exposes no cross-lane controls', async () => {
  window.localStorage.clear()
  const view = await renderClient(<ReorderableOverview cards={cards}/>)
  try {
    const card = document.querySelector<HTMLElement>('[data-overview-card="a"]')!
    card.focus()
    await act(async () => card.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', altKey: true, bubbles: true })))
    assert.equal(document.activeElement?.getAttribute('aria-label'), 'Reorder a')
    assert.deepEqual(lane('telemetry'), ['b', 'a'])
    assert.equal(document.querySelector('[aria-label*="to insights"]'), null)
    assert.deepEqual(lane('insights'), ['r', 's'])
  } finally { await view.unmount(); window.localStorage.clear() }
})

test('pointer capture failure clears pending state so a later drag can start', async () => {
  window.localStorage.clear()
  const view = await renderClient(<ReorderableOverview cards={cards}/>)
  try {
    const handle = document.querySelector<HTMLElement>('[data-overview-card="a"]')!
    let attempts = 0
    const releases: number[] = []
    Object.defineProperties(handle, {
      setPointerCapture: { configurable: true, value: () => { attempts += 1; if (attempts === 1) throw new DOMException('Inactive pointer', 'NotFoundError') } },
      hasPointerCapture: { configurable: true, value: () => attempts > 1 },
      releasePointerCapture: { configurable: true, value: (pointerId: number) => releases.push(pointerId) },
    })

    await act(async () => handle.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, button: 0, pointerId: 1, clientX: 10, clientY: 10 })))
    assert.equal(attempts, 1)
    assert.equal(document.querySelector('[role="status"]')?.textContent, 'Could not start pointer drag for a. Use Alt+Arrow keys instead.')

    await act(async () => handle.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, button: 0, pointerId: 2, clientX: 10, clientY: 10 })))
    assert.equal(attempts, 2)
    await act(async () => handle.dispatchEvent(new PointerEvent('pointercancel', { bubbles: true, pointerId: 2 })))
    assert.deepEqual(releases, [2])
  } finally { await view.unmount(); window.localStorage.clear() }
})

test('Escape and unmount release an owned pointer capture', async () => {
  window.localStorage.clear()
  const view = await renderClient(<ReorderableOverview cards={cards}/>)
  const handle = document.querySelector<HTMLElement>('[data-overview-card="a"]')!
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

test('legacy saved orders cannot move cards across the mock-defined lanes', async () => {
  window.localStorage.clear()
  window.localStorage.setItem(key, JSON.stringify({ order: ['b', 'r', 'a', 's'], widths: { b: true }, lanes: { b: 'insights', r: 'telemetry' } }))
  let view = await renderClient(<ReorderableOverview cards={cards}/>)
  try {
    assert.deepEqual(lane('telemetry'), ['b', 'a'])
    assert.deepEqual(lane('insights'), ['r', 's'])
    assert.equal(document.querySelector('[aria-label*="to insights"]'), null)
    assert.equal(document.querySelector('[data-overview-card="b"] [aria-label="Toggle width"]')?.getAttribute('aria-pressed'), 'true')
    await view.unmount()
    view = await renderClient(<ReorderableOverview cards={cards}/>)
    assert.deepEqual(lane('telemetry'), ['b', 'a'])
    assert.deepEqual(lane('insights'), ['r', 's'])
    assert.equal(document.querySelectorAll('[data-card="b"]').length, 1)
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
