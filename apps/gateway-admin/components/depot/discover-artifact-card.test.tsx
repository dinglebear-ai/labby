import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { DiscoverArtifactCard } from './discover-artifact-card'

test('cards distinguish known zero, singular, plural and unknown file counts', () => {
  for (const compact of [false, true]) {
    for (const fileCount of [undefined, 0, 1, 2000]) {
      const markup = renderToStaticMarkup(<DiscoverArtifactCard artifact={{
        providerId: 'catalog', artifactId: 'files', currentRevision: { fileCount },
      }} compact={compact} selected={false} href="/discover" />)
      if (fileCount === undefined) assert.doesNotMatch(markup, />\d+ files?</)
      else assert.match(markup, new RegExp(`>${fileCount} ${fileCount === 1 ? 'file' : 'files'}<`))
    }
  }
})

test('card shows supplied source format while policy and revision details stay in the inspector', () => {
  for (const compact of [false, true]) {
    const markup = renderToStaticMarkup(<DiscoverArtifactCard artifact={{
      providerId: 'catalog', artifactId: 'formatted', kind: 'skill',
      provenance: { originalFormat: 'agent-skill', originalVersion: '1' },
      publication: { visibility: 'public', distribution: 'allowed' }, revisionCount: 7,
    }} compact={compact} selected={false} href="/depot" />)
    assert.match(markup, /title="Source format"><svg[\s\S]*?<\/svg>agent-skill<\/span>/)
    assert.doesNotMatch(markup, />public<|>allowed<|7 revisions/)
    const absent = renderToStaticMarkup(<DiscoverArtifactCard artifact={{
      providerId: 'github', artifactId: 'unknown-format', kind: 'skill',
    }} compact={compact} selected={false} href="/depot" />)
    assert.doesNotMatch(absent, /title="Source format"|SKILL\.md|progressive/)
  }
})

test('card keeps source identity and descriptor title without inventing verification or popularity', () => {
  const markup = renderToStaticMarkup(<DiscoverArtifactCard artifact={{
    providerId: 'catalog', artifactId: 'shared/id',
    descriptor: { kind: 'skill', title: 'Review changes', namespace: 'team' },
    currentRevision: { authoredAt: '2026-09-08T10:00:00Z' },
  }} compact={false} selected={true} href="/depot?artifactProvider=catalog&artifact=shared%2Fid" />)
  assert.match(markup, /Review changes/)
  assert.match(markup, /Capability/)
  assert.match(markup, /catalog/)
  assert.match(markup, /dateTime="2026-09-08T10:00:00Z"/)
  assert.match(markup, /aria-current="page"/)
  assert.match(markup, /artifactProvider=catalog/)
  assert.doesNotMatch(markup, />Inspect[\s<]|>shared\/id</)
  assert.match(markup, /data-artifact-key=/)
  assert.doesNotMatch(markup, /VERIFIED|Installs|Stars|Forks/)
})

test('compact cards handle unknown kinds, absent publisher and invalid dates', () => {
  const markup = renderToStaticMarkup(<DiscoverArtifactCard artifact={{
    providerId: 'private', artifactId: 'unknown', kind: 'future-kind',
    currentRevision: { authoredAt: 'not-a-date' },
  }} compact={true} selected={false} href="/depot?artifact=unknown&artifactProvider=private" />)
  assert.match(markup, /data-density="compact"/)
  assert.match(markup, /future-kind/)
  assert.match(markup, /Publisher not supplied/)
  assert.doesNotMatch(markup, /<time|aria-current/)
})

test('card displays a relative revision age without losing its exact timestamp', () => {
  const markup = renderToStaticMarkup(<DiscoverArtifactCard artifact={{
    providerId: 'catalog', artifactId: 'dated',
    currentRevision: { authoredAt: '2026-09-08T10:00:00Z' },
  }} compact={false} selected={false} href="/depot?artifact=dated&artifactProvider=catalog" now={Date.parse('2026-09-08T14:00:00Z')} />)
  assert.match(markup, />4h ago<\/time>/)
  assert.match(markup, /title="Revision authored 2026-09-08T10:00:00Z"/)
  assert.match(markup, /dateTime="2026-09-08T10:00:00Z"/)
})

test('grid card uses the inset kind stripe and compact header geometry', () => {
  const markup = renderToStaticMarkup(<DiscoverArtifactCard artifact={{ providerId: 'catalog', artifactId: 'mcp', kind: 'mcp', title: 'Server' }} compact={false} selected={false} href="/depot?artifact=mcp" />)
  assert.match(markup, /data-kind-stripe="true"/)
  assert.match(markup, /bottom-3\.5 left-0 top-3\.5 w-0\.5/)
  assert.doesNotMatch(markup, /border-l-2|border-left-color/)
  assert.match(markup, /size-8 shrink-0 place-items-center/)
  assert.match(markup, /px-4 pt-3\.5 pb-\[13px\]/)
  assert.doesNotMatch(markup, /Verified|Installs|Stars/)
})

test('reported verification and safe metrics render without inferring missing evidence', () => {
  const base = { providerId: 'catalog', artifactId: 'reported', title: 'Reported' }
  const render = (extra: object) => renderToStaticMarkup(<DiscoverArtifactCard artifact={{ ...base, ...extra }} compact={false} selected={false} href="/depot" />)
  const known = render({ publisherVerified: true, metrics: { stars: 0, installs: 12500, forks: 3 } })
  assert.match(known, /Publisher verified/)
  assert.match(known, />Verified</)
  assert.match(known, /aria-label="0 stars"/)
  assert.match(known, /aria-label="12500 installs"/)
  assert.match(known, />12\.5K</)
  assert.match(known, /aria-label="3 forks"/)
  for (const extra of [{}, { publisherVerified: false }, { publisherVerified: 'true' }, { metrics: { stars: -1, installs: Infinity, forks: 1.5 } }]) {
    assert.doesNotMatch(render(extra), /Publisher verified|>Verified<|aria-label="[^"]+ (stars|installs|forks)"/)
  }
})
