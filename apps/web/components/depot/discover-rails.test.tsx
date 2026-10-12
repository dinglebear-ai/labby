import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import type { FederatedArtifact } from '@/lib/api/depot-client'
import { DiscoverRails, discoverReferenceRails } from './discover-rails'

const now = Date.parse('2026-10-03T12:00:00Z')
const artifacts: FederatedArtifact[] = [
  { providerId: 'catalog-a', artifactId: 'old', kind: 'skill', title: 'Older skill', namespace: 'tootie.tv', currentRevision: { authoredAt: '2026-10-01T12:00:00Z' }, metrics: { installs: 999999 } },
  { providerId: 'catalog-a', artifactId: 'new', kind: 'agent', title: 'Latest agent', namespace: 'unrelated', currentRevision: { authoredAt: '2026-10-03T11:00:00Z' } },
  { providerId: 'catalog-b', artifactId: 'loadout', descriptor: { kind: 'loadout', title: 'Operations loadout' }, currentRevision: { authoredAt: '2026-10-02T12:00:00Z' } },
  { providerId: 'catalog-b', artifactId: 'undated', kind: 'skill', title: 'Undated skill' },
  { providerId: 'catalog-c', artifactId: 'future', kind: 'plugin', currentRevision: { authoredAt: '2099-01-01' } },
  { providerId: 'catalog-c', artifactId: 'invalid', currentRevision: { authoredAt: 'invalid' } },
]
const artifactHref = (providerId?: string, id?: string) => `/depot/?artifactProvider=${providerId}&artifact=${id}`

test('Discover collections use only retrieved revision dates, source identities and loadout kinds', () => {
  const rails = discoverReferenceRails(artifacts, now)
  assert.deepEqual(rails.map(rail => rail.label), ['Recently updated', 'From connected sources', 'Loadouts to explore'])
  assert.deepEqual(rails[0].items.map(item => item.artifactId), ['new', 'loadout', 'old'])
  assert.deepEqual(rails[1].items.map(item => item.providerId), ['catalog-a', 'catalog-b', 'catalog-c'])
  assert.deepEqual(rails[2].items.map(item => item.artifactId), ['loadout'])
  assert.equal(artifacts[0].artifactId, 'old', 'selection must not mutate catalog order')
})

test('Discover collections bound cards without claiming popularity, team membership or affinity', () => {
  const html = renderToStaticMarkup(<DiscoverRails artifacts={artifacts} artifactHref={artifactHref} now={now} />)
  for (const title of ['Latest agent', 'Operations loadout', 'Older skill']) assert.ok(html.includes(title))
  assert.ok(html.includes('artifactProvider=catalog-b'))
  assert.match(html, /currently loaded catalog results/)
  assert.doesNotMatch(html, /Popular This Week|New From Your Team|Pairs With Your Loadouts|Recommendation evidence|installs, last 7 days/)
  const many = Array.from({ length: 30 }, (_, i) => ({ providerId: `source-${i}`, artifactId: String(i), kind: 'loadout', currentRevision: { authoredAt: '2026-10-02' } }))
  assert.deepEqual(discoverReferenceRails(many, now).map(rail => rail.items.length), [8, 8, 8])
})

test('Discover collections distinguish initial loading, confirmed empty and unavailable catalogs', () => {
  const props = { artifacts: [], artifactHref, now }
  const loading = renderToStaticMarkup(<DiscoverRails {...props} loading />)
  assert.match(loading, /Checking connected sources/)
  assert.doesNotMatch(loading, /No artifacts|No loadouts/)
  const empty = renderToStaticMarkup(<DiscoverRails {...props} />)
  assert.match(empty, /No artifacts returned/)
  assert.match(empty, /Publish artifact/)
  const unavailable = renderToStaticMarkup(<DiscoverRails {...props} incomplete failureMessage="The catalog index is still preparing." onRetry={() => {}} />)
  assert.match(unavailable, /Catalog results unavailable/)
  assert.match(unavailable, /catalog index is still preparing/)
  assert.match(unavailable, /Retry search/)
  assert.doesNotMatch(unavailable, /No artifacts returned/)
})

test('Discover collections retain successful partial-source data and explain missing fields honestly', () => {
  const html = renderToStaticMarkup(<DiscoverRails artifacts={[artifacts[3]]} artifactHref={artifactHref} now={now} incomplete failureMessage="One source is unavailable." onRetry={() => {}} />)
  assert.match(html, /Undated skill/)
  assert.match(html, /One source is unavailable/)
  assert.doesNotMatch(html, /Recently updated|No revision dates reported in these results/)
  assert.doesNotMatch(html, /Loadouts to explore|No loadouts in these results/)
  assert.doesNotMatch(html, /No artifacts returned|Catalog results unavailable/)
})

test('Discover collection retry invokes the current catalog request', async () => {
  const dom = installTestDom()
  Object.defineProperty(globalThis, 'self', { value: dom, configurable: true })
  let retries = 0
  const view = await renderClient(<DiscoverRails artifacts={[]} artifactHref={artifactHref} incomplete failureMessage="Source unavailable." onRetry={() => { retries++ }} />)
  try {
    const retry = [...view.container.querySelectorAll('button')].find(button => button.textContent?.includes('Retry search'))
    assert.ok(retry)
    await act(async () => retry.click())
    assert.equal(retries, 1)
  } finally { await view.unmount() }
})
