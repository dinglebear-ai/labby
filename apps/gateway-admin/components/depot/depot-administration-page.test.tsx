import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { DepotAdministrationPage, OperationGrid } from './depot-administration-page'

test('administration uses compact attached workspace navigation and visible header actions', () => {
  const html = renderToStaticMarkup(<DepotAdministrationPage />)
  const nav = html.match(/<nav aria-label="Depot administration workspaces"[\s\S]*?<\/nav>/)?.[0]
  assert.ok(nav)
  assert.match(nav, /aurora-scrollbar/)
  assert.match(nav, /h-\[38px\]/)
  assert.match(nav, /rounded-b-aurora-3/)
  assert.equal((nav.match(/<button/g) ?? []).length, 4)
  assert.match(nav, /aria-current="page"/)
  assert.match(html, /href="\/settings\/depot\/"/)
  assert.match(html, /Authority unavailable/)
  assert.doesNotMatch(html, /Authority connected/)
})

test('operation catalog groups search and dense cards while retaining safety labels', () => {
  const operations = [true, false].map((destructive, index) => ({ name: `fixture.operation.${index}`, title: `Operation ${index}`, description: 'Synthetic test operation', group: 'access' as const, inputSchema: { type: 'object' as const }, annotations: { readOnlyHint: !destructive, destructiveHint: destructive } }))
  const html = renderToStaticMarkup(<OperationGrid workspace="access" operations={operations} />)
  assert.match(html, /aria-label="access operation catalog"/)
  assert.match(html, /Search access operations/)
  assert.match(html, /p-3\.5/)
  assert.match(html, /font-\[760\]/)
  assert.match(html, />Destructive</)
  assert.match(html, />Read</)
  assert.match(html, /currentColor_30%,transparent/)
  assert.match(html, /currentColor_11%,transparent/)
  assert.match(html, /text-pretty/)
  assert.doesNotMatch(html, /hover:-translate-y/)
})

test('all administration operation workspaces use the same grid and isolate their operations', () => {
  const groups = ['catalog', 'access', 'operations'] as const
  const operations = groups.map(group => ({ name: `fixture.${group}`, title: `${group} action`, description: 'Synthetic operation', group, inputSchema: { type: 'object' as const }, annotations: { readOnlyHint: true } }))
  for (const workspace of groups) {
    const html = renderToStaticMarkup(<OperationGrid workspace={workspace} operations={operations} />)
    assert.ok(html.includes(`aria-label="${workspace} operation catalog"`))
    assert.ok(html.includes(`fixture.${workspace}`))
    assert.match(html, /1 operations/)
    for (const other of groups.filter(group => group !== workspace)) assert.ok(!html.includes(`fixture.${other}`))
  }
})
