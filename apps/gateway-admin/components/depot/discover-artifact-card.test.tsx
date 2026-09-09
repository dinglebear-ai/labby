import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { DiscoverArtifactCard } from './discover-artifact-card'

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
