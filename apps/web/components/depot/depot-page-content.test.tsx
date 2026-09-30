import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { DepotClientError, type DepotArtifact } from '@/lib/api/depot-client'
import { ArtifactResults, depotCoveragePulse, discoveryCountLabel, exactImportConnection, exactImportParams, mergeArtifactPages, discoveryFailureLabel, discoveryErrorState } from './depot-page-content'

test('coverage failures expose a safe provider summary without raw failure kinds', () => {
  assert.equal(discoveryFailureLabel({ providerId: 'catalog', kind: 'unsupported_kind' }), 'catalog: kind filter unsupported')
  assert.equal(discoveryFailureLabel({ providerId: 'catalog', kind: 'unexpected_secret_bearing_detail' }), 'catalog: source unavailable')
})

test('catalog authentication failures remain distinct from connection failures', () => {
  assert.equal(discoveryErrorState(new DepotClientError(401, 'unauthorized', 'secret')), 'auth')
  assert.equal(discoveryErrorState(new DepotClientError(403, 'forbidden', 'secret')), 'auth')
  assert.equal(discoveryErrorState(new DepotClientError(503, 'unavailable', 'secret')), 'unavailable')
})

test('unavailable catalog counts remain unknown rather than implying an empty catalog', () => {
  assert.equal(discoveryCountLabel(0, false, true), '—')
  assert.equal(discoveryCountLabel(50, true, true), '—')
  assert.equal(discoveryCountLabel(0, true, false), '0')
  assert.equal(discoveryCountLabel(50, false, false), '≥ 50')
})

test('depotCoveragePulse never renders failed provider coverage as healthy', () => {
  assert.deepEqual(depotCoveragePulse('all_failed'), {
    color: 'var(--aurora-error)',
    label: 'all_failed',
  })
  assert.deepEqual(depotCoveragePulse('partial'), {
    color: 'var(--aurora-warn)',
    label: 'partial',
  })
  assert.deepEqual(depotCoveragePulse('complete'), {
    color: 'var(--aurora-success)',
    label: 'complete',
  })
  assert.deepEqual(depotCoveragePulse(), {
    color: 'var(--aurora-text-muted)',
    label: 'status unknown',
  })
})

test('mergeArtifactPages appends unique cursor results in order', () => {
  const current: DepotArtifact[] = [
    { id: 'artifact-a', title: 'A' },
    { descriptor: { id: 'artifact-b', title: 'B' } },
  ]
  const incoming: DepotArtifact[] = [
    { id: 'artifact-b', title: 'Duplicate B' },
    { id: 'artifact-c', title: 'C' },
    { descriptor: { id: 'artifact-d', title: 'D' } },
    { id: 'artifact-c', title: 'Duplicate C' },
  ]

  assert.deepEqual(
    mergeArtifactPages(current, incoming).map(artifact => artifact.id ?? artifact.descriptor?.id),
    ['artifact-a', 'artifact-b', 'artifact-c', 'artifact-d'],
  )
})

test('mergeArtifactPages drops cursor rows without a stable artifact identity', () => {
  assert.deepEqual(mergeArtifactPages([], [{ title: 'Missing identity' }]), [])
})

test('Discover result states preserve loading, empty, and partial-provider explanations', () => {
  const base = {
    artifacts: [], activeQuery: '', view: 'cards' as const, density: 'comfortable' as const,
    artifactHref: () => '/depot/', onReset: () => {}, selectionMode: false, selectedBulkKeys: [], cursorIndex: -1,
    onToggleSelected: () => {}, onEnterSelectionMode: () => {}, onAdd: async () => {}, isInLibrary: () => false, actionPending: false,
  }
  const loading = renderToStaticMarkup(<ArtifactResults {...base} loading incomplete={false}/>)
  assert.match(loading, /Searching catalog…/)
  const empty = renderToStaticMarkup(<ArtifactResults {...base} loading={false} incomplete={false}/>)
  assert.match(empty, /No artifacts match that filter./)
  assert.match(empty, /Clear the kind and source filters, or publish the first one./)
  assert.match(empty, /Reset Filters/)
  const partial = renderToStaticMarkup(<ArtifactResults {...base} loading={false} incomplete/>)
  assert.match(partial, /Search results are not complete yet./)
  assert.match(partial, /Retry once every source is available./)
  assert.doesNotMatch(partial, /Reset Filters/)
})

test('exactImportParams preserves the selected provider, artifact, revision, library version, and idempotency key', () => {
  const params = exactImportParams(
    { providerId: 'team-depot', artifactId: 'artifact-1', currentRevisionId: 'revision-7' },
    [{ id: 'team-depot' }],
    12,
    'depot-import-test-key',
  )
  assert.deepEqual(params, {
    source: { kind: 'depot', connection_id: 'team-depot', artifact_id: 'artifact-1', revision_id: 'revision-7' },
    expected_library_version: 12,
    idempotency_key: 'depot-import-test-key',
  })
  assert.throws(() => exactImportParams(
    { providerId: 'team-depot', artifactId: 'artifact-1' },
    [{ id: 'team-depot' }],
    12,
    'key',
  ), /exact Artifact and revision identity/)
  assert.throws(() => exactImportParams(
    { providerId: 'team-depot', artifactId: 'artifact-1', currentRevisionId: 'revision-7' },
    [{ id: 'team-depot' }],
    '12',
    'key',
  ), /valid current library version/)
})

test('exactImportConnection requires a source connection matching the discovery provider', () => {
  assert.equal(exactImportConnection('team-depot', [{ id: 'public-depot' }, { id: 'team-depot' }]), 'team-depot')
  assert.throws(
    () => exactImportConnection('team-depot', [{ id: 'different-depot' }]),
    /Configure an Artifact acquisition connection named “team-depot”/,
  )
})
