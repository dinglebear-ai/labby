import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { DiscoverFeaturedRail } from './discover-featured-rail'

const artifact = { providerId: 'team', artifactId: 'one', title: 'Research workspace', kind: 'loadout', namespace: 'fixture-team' }
const props = { title: 'Popular This Week', subtitle: 'installs, last 7 days', artifactHref: (provider: string, id: string) => `/depot?artifactProvider=${provider}&artifact=${id}` }

test('featured cards retain exact source identity and use shared kind styling', () => {
  const html = renderToStaticMarkup(<DiscoverFeaturedRail {...props} feed={{ state: 'ready', items: [{ artifact, installs: 58000 }] }} />)
  assert.match(html, /artifactProvider=team&amp;artifact=one/)
  assert.match(html, /Research workspace/)
  assert.match(html, /var\(--aurora-success\)/)
  assert.match(html, /aria-label="58000 installs"/)
  assert.match(html, />58K</)
  assert.match(html, /snap-x/)
  assert.match(html, /gap-\[9px\]/)
  assert.match(html, /size-\[11px\]/)
  assert.match(html, /text-pretty/)
  assert.doesNotMatch(html, /min-h-\[138px\]/)
})

test('missing and invalid metrics are not invented while known zero remains visible', () => {
  for (const installs of [undefined, -1, NaN, Infinity, 1.5, Number.MAX_SAFE_INTEGER + 1]) {
    const html = renderToStaticMarkup(<DiscoverFeaturedRail {...props} feed={{ state: 'ready', items: [{ artifact, installs }] }} />)
    assert.doesNotMatch(html, /aria-label="[^"]* installs"/)
  }
  assert.match(renderToStaticMarkup(<DiscoverFeaturedRail {...props} feed={{ state: 'ready', items: [{ artifact, installs: 0 }] }} />), /aria-label="0 installs"/)
})

test('unavailable, loading, and empty feeds remain distinct with no placeholder artifacts', () => {
  for (const [feed, expected] of [
    [{ state: 'loading' }, 'Loading popular this week'],
    [{ state: 'unavailable', message: 'Install activity is not reported.' }, 'Install activity is not reported.'],
    [{ state: 'ready', items: [] }, 'No artifacts in this feed yet.'],
  ] as const) {
    const html = renderToStaticMarkup(<DiscoverFeaturedRail {...props} feed={feed} />)
    assert.ok(html.includes(expected))
    assert.doesNotMatch(html, /<a /)
  }
})
test('featured verification is displayed only for an explicit true value', () => {
  for (const publisherVerified of [undefined, false, true]) {
    const html = renderToStaticMarkup(<DiscoverFeaturedRail {...props} feed={{ state: 'ready', items: [{ artifact: { ...artifact, publisherVerified } }] }} />)
    assert.equal(html.includes('aria-label="Publisher verified"'), publisherVerified === true)
  }
})
