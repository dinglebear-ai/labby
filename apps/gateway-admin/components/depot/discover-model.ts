import type { FederatedArtifact } from '@/lib/api/depot-client'

export type DiscoverySort = 'relevance' | 'newest' | 'name' | 'installs' | 'stars' | 'forks' | 'verified'
export type DiscoveryShelf = 'trending' | 'new' | 'popular' | 'bundled' | 'forks' | 'curated'

export const DISCOVERY_SHELVES: ReadonlyArray<{ id: DiscoveryShelf; label: string; title: string; hint: string }> = [
  { id: 'trending', label: 'Trending', title: 'Trending This Week', hint: 'velocity of installs + forks' },
  { id: 'new', label: 'New', title: 'Newly Published', hint: 'first seen in the last 7 days' },
  { id: 'popular', label: 'Popular', title: 'Most Installed', hint: 'all-time installs across every target' },
  { id: 'bundled', label: 'Bundled', title: 'Commonly Bundled', hint: 'artifacts that ship together in loadouts' },
  { id: 'forks', label: 'Hot Forks', title: 'Hot Forks', hint: 'forks moving faster than upstream' },
  { id: 'curated', label: 'Curated', title: 'Curated by Depot', hint: 'verified publishers, reviewed by hand' },
]

export function artifactKind(artifact: FederatedArtifact): string {
  return artifact.kind ?? artifact.descriptor?.kind ?? 'artifact'
}

export function artifactTitle(artifact: FederatedArtifact): string {
  return artifact.title || artifact.descriptor?.title || artifact.name || artifact.descriptor?.name || artifact.artifactId
}

/** Explain only matches that can be proven from fields present in the retained result. */
export function artifactMatchLabel(artifact: FederatedArtifact, query: string): string | undefined {
  const needle = query.trim().toLowerCase()
  if (!needle) return undefined
  const includes = (value?: string | null) => typeof value === 'string' && value.toLowerCase().includes(needle)
  if ([artifact.name, artifact.title, artifact.descriptor?.name, artifact.descriptor?.title, artifact.artifactId].some(includes)) return 'matched in name'
  if (artifact.descriptor?.tags?.some(tag => includes(tag))) return 'matched in tags'
  if ([artifact.namespace, artifact.descriptor?.namespace].some(includes)) return 'matched in publisher'
  if ([artifact.kind, artifact.descriptor?.kind].some(includes)) return 'matched in kind'
  if (includes(artifact.description ?? artifact.descriptor?.description)) return 'matched in description'
  if (artifact.readme?.state === 'available' && includes(artifact.readme.content)) return 'matched in readme'
  return undefined
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

/** The reference Bazaar shelves are presentation lenses over already-authorized results. */
export function selectDiscoveryShelf(artifacts: FederatedArtifact[], shelf: DiscoveryShelf): FederatedArtifact[] {
  let selected = [...artifacts]
  if (shelf === 'curated') selected = selected.filter(artifact => artifact.publisherVerified === true)
  if (shelf === 'forks') selected = selected.filter(artifact => (artifact.metrics?.forks ?? 0) > 60)
  if (shelf === 'trending' || shelf === 'forks') selected.sort((a, b) => (b.metrics?.forks ?? -1) - (a.metrics?.forks ?? -1))
  if (shelf === 'new') selected.sort((a, b) => revisionTime(b) - revisionTime(a))
  if (shelf === 'popular') selected.sort((a, b) => (b.metrics?.installs ?? -1) - (a.metrics?.installs ?? -1))
  if (shelf === 'bundled') selected.sort((a, b) => {
    const bundleRank = (artifact: FederatedArtifact) => ['loadout', 'plugin', 'agent-plugin', 'apm-package'].includes(artifactKind(artifact)) ? 1 : 0
    return bundleRank(b) - bundleRank(a) || (b.metrics?.stars ?? -1) - (a.metrics?.stars ?? -1)
  })
  return selected
}

/** Presentation ordering is limited to retained results, never a global ranking. */
export function selectDiscoveryResults(
  artifacts: FederatedArtifact[], sort: DiscoverySort,
): FederatedArtifact[] {
  const selected = [...artifacts]
  if (sort === 'name') selected.sort((a, b) => artifactTitle(a).localeCompare(artifactTitle(b)))
  if (sort === 'newest') selected.sort((a, b) => revisionTime(b) - revisionTime(a))
  if (sort === 'installs' || sort === 'forks' || sort === 'stars') selected.sort((a, b) => (b.metrics?.[sort] ?? -1) - (a.metrics?.[sort] ?? -1))
  if (sort === 'verified') selected.sort((a, b) => Number(b.publisherVerified === true) - Number(a.publisherVerified === true))
  return selected
}

function revisionTime(artifact: FederatedArtifact): number {
  const value = Date.parse(artifact.currentRevision?.authoredAt ?? '')
  return Number.isFinite(value) ? value : 0
}
