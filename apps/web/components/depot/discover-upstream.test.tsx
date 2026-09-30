import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { DiscoverUpstream } from './discover-upstream'

test('upstream links preserve exact provider authority without inventing sync state', () => {
  const html = renderToStaticMarkup(<DiscoverUpstream artifact={{ providerId: 'team-source', artifactId: 'child', lineage: {
    upstreamArtifactId: 'parent', upstreamRevisionId: 'revision-1', forkedFromArtifactId: 'origin', forkedFromRevisionId: null, following: true, lastObservedUpstreamRevisionId: 'revision-2',
  } }} />)
  assert.match(html, /artifactProvider=team-source&amp;artifact=parent/)
  assert.match(html, /artifactProvider=team-source&amp;artifact=origin/)
  assert.match(html, /Following upstream/)
  assert.match(html, /does not indicate synchronization status/)
  assert.doesNotMatch(html, /https:|Update available|Up to date/)
})

test('omitted or empty lineage reveals no inaccessible relationship', () => {
  assert.equal(renderToStaticMarkup(<DiscoverUpstream artifact={{ providerId: 'source', artifactId: 'child' }} />), '')
  assert.equal(renderToStaticMarkup(<DiscoverUpstream artifact={{ providerId: 'source', artifactId: 'child', lineage: { upstreamArtifactId: null, upstreamRevisionId: null, forkedFromArtifactId: null, forkedFromRevisionId: null, following: false, lastObservedUpstreamRevisionId: null } }} />), '')
})
