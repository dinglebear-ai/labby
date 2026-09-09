import type { FederatedArtifact } from '@/lib/api/depot-client'

export type DiscoverySort = 'catalog' | 'newest' | 'name'

export function artifactKind(artifact: FederatedArtifact): string {
  return artifact.kind ?? artifact.descriptor?.kind ?? 'artifact'
}

export function artifactTitle(artifact: FederatedArtifact): string {
  return artifact.title || artifact.descriptor?.title || artifact.name || artifact.descriptor?.name || artifact.artifactId
}

/** Keep server rendering deterministic; future dates are not recent activity. */
export function revisionAge(timestamp: string, now?: number): string {
  const authored = Date.parse(timestamp)
  if (!Number.isFinite(authored)) return ''
  const absolute = timestamp.slice(0, 10)
  if (now === undefined || !Number.isFinite(now) || authored > now) return absolute
  const seconds = Math.floor((now - authored) / 1000)
  if (seconds < 60) return 'just now'
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`
  if (seconds < 604800) return `${Math.floor(seconds / 86400)}d ago`
  if (seconds < 2592000) return `${Math.floor(seconds / 604800)}w ago`
  return absolute
}

/** Presentation ordering is limited to retained results, never a global ranking. */
export function selectDiscoveryResults(
  artifacts: FederatedArtifact[], sort: DiscoverySort,
): FederatedArtifact[] {
  const selected = [...artifacts]
  if (sort === 'name') selected.sort((a, b) => artifactTitle(a).localeCompare(artifactTitle(b)))
  if (sort === 'newest') selected.sort((a, b) => revisionTime(b) - revisionTime(a))
  return selected
}

function revisionTime(artifact: FederatedArtifact): number {
  const value = Date.parse(artifact.currentRevision?.authoredAt ?? '')
  return Number.isFinite(value) ? value : 0
}
