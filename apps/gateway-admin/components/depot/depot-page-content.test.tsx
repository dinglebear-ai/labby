import assert from 'node:assert/strict'
import test from 'node:test'

import type { DepotArtifact } from '@/lib/api/depot-client'
import { depotCoveragePulse, discoveryCountLabel, exactImportConnection, mergeArtifactPages } from './depot-page-content'

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

test('exactImportConnection requires a source connection matching the discovery provider', () => {
  assert.equal(exactImportConnection('team-depot', [{ id: 'public-depot' }, { id: 'team-depot' }]), 'team-depot')
  assert.throws(
    () => exactImportConnection('team-depot', [{ id: 'different-depot' }]),
    /Configure an Artifact acquisition connection named “team-depot”/,
  )
})
