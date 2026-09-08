import type { FederatedArtifact } from '@/lib/api/depot-client'

export type DiscoverySort = 'catalog' | 'newest' | 'name'

export function artifactKind(artifact: FederatedArtifact): string {
  return artifact.kind ?? artifact.descriptor?.kind ?? 'artifact'
}

export function artifactTitle(artifact: FederatedArtifact): string {
  return artifact.title || artifact.descriptor?.title || artifact.name || artifact.descriptor?.name || artifact.artifactId
}

/** Presentation ordering is limited to retained results, never a global ranking. */
export function selectDiscoveryResults(
  artifacts: FederatedArtifact[], kind: string, sort: DiscoverySort,
): FederatedArtifact[] {
  const selected = artifacts.filter(artifact => kind === 'all' || artifactKind(artifact) === kind)
  if (sort === 'name') selected.sort((a, b) => artifactTitle(a).localeCompare(artifactTitle(b)))
  if (sort === 'newest') selected.sort((a, b) => revisionTime(b) - revisionTime(a))
  return selected
}

function revisionTime(artifact: FederatedArtifact): number {
  const value = Date.parse(artifact.currentRevision?.authoredAt ?? '')
  return Number.isFinite(value) ? value : 0
}
