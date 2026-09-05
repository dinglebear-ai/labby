import assert from 'node:assert/strict'
import test from 'node:test'

import type { DepotArtifact } from '@/lib/api/depot-client'
import { mergeArtifactPages } from './depot-page-content'

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
