import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { DiscoverHighlights } from './discover-highlights'

test('unsupported highlights retain mock sections without invented activity or artifacts', () => {
  const html = renderToStaticMarkup(<DiscoverHighlights artifactHref={() => '/depot'} />)
  for (const heading of ['Popular This Week', 'New From Your Team', 'Pairs With Your Loadouts']) assert.ok(html.includes(heading))
  assert.equal((html.match(/not available from the connected catalog/g) ?? []).length, 3)
  assert.doesNotMatch(html, /<a |Loading|No artifacts in this feed/)
})

test('each highlight accepts its own independently qualified feed state', () => {
  const html = renderToStaticMarkup(<DiscoverHighlights artifactHref={(provider, id) => `/depot?artifactProvider=${provider}&artifact=${id}`} feeds={{
    popular: { state: 'loading' },
    team: { state: 'ready', items: [{ artifact: { providerId: 'team', artifactId: 'qualified', title: 'Team artifact', kind: 'agent' } }] },
    loadouts: { state: 'ready', items: [] },
  }} />)
  assert.match(html, /Loading popular this week/)
  assert.match(html, /artifactProvider=team&amp;artifact=qualified/)
  assert.match(html, /No artifacts in this feed yet/)
  assert.doesNotMatch(html, /aria-label="[^"]* installs"/)
})
