import type { DepotArtifact } from '@/lib/api/depot-client'

export type LibraryKind = 'all' | string
export type LibrarySort = 'catalog' | 'name' | 'kind'
export type ArtifactType = 'mcp' | 'acp' | 'agent' | 'skill' | 'command' | 'plugin' | 'marketplace' | 'prompt'

const KIND_ALIASES: Record<string, ArtifactType> = {
  mcp_server: 'mcp', mcpserver: 'mcp',
  acp_agent: 'acp',
  agents: 'agent', skills: 'skill', commands: 'command', plugins: 'plugin', marketplaces: 'marketplace', prompts: 'prompt',
}

export function artifactId(artifact: DepotArtifact): string {
  return artifact.id ?? artifact.descriptor?.id ?? ''
}

export function artifactKind(artifact: DepotArtifact): string {
  const kind = (artifact.kind ?? artifact.descriptor?.kind ?? 'artifact').toLocaleLowerCase().replace(/[ -]+/g, '_')
  return Object.hasOwn(KIND_ALIASES, kind) ? KIND_ALIASES[kind] : kind
}

export function artifactLabel(artifact: DepotArtifact): string {
  return artifact.title ?? artifact.descriptor?.title ?? artifact.name ?? artifact.descriptor?.name ?? artifactId(artifact)
}

export function artifactDescription(artifact: DepotArtifact): string {
  return artifact.description ?? artifact.descriptor?.description ?? 'No description supplied.'
}

export function collectArtifactKinds(artifacts: DepotArtifact[]): string[] {
  return [...new Set(artifacts.map(artifactKind))].sort((a, b) => a.localeCompare(b))
}

export function filterArtifacts(artifacts: DepotArtifact[], kind: LibraryKind): DepotArtifact[] {
  if (kind === 'all') return artifacts
  return artifacts.filter((artifact) => artifactKind(artifact) === kind)
}

/** Loaded-result facets only; these counts make no claim about unseen pages. */
export function collectArtifactTags(artifacts: DepotArtifact[]): Array<{ tag: string; count: number }> {
  const counts = new Map<string, number>()
  for (const artifact of artifacts) {
    for (const tag of new Set(artifact.descriptor?.tags ?? [])) {
      counts.set(tag, (counts.get(tag) ?? 0) + 1)
    }
  }
  return [...counts].map(([tag, count]) => ({ tag, count }))
    .sort((a, b) => a.tag.localeCompare(b.tag, 'en'))
}

export function filterLibraryArtifacts(artifacts: DepotArtifact[], kind: LibraryKind, tag?: string | null): DepotArtifact[] {
  const matchingKind = filterArtifacts(artifacts, kind)
  return tag == null ? matchingKind : matchingKind.filter(artifact => artifact.descriptor?.tags?.includes(tag))
}

/** Presentation ordering never mutates the retained server order. */
export function sortLibraryArtifacts(artifacts: DepotArtifact[], sort: LibrarySort): DepotArtifact[] {
  const ordered = [...artifacts]
  if (sort === 'catalog') return ordered
  return ordered.sort((a, b) => {
    if (sort === 'kind') {
      const kindOrder = artifactKind(a).localeCompare(artifactKind(b), 'en')
      if (kindOrder) return kindOrder
    }
    return artifactLabel(a).localeCompare(artifactLabel(b), 'en')
  })
}

export function artifactExportFilename(artifact: DepotArtifact): string {
  const base = (artifact.name ?? artifact.descriptor?.name ?? artifactLabel(artifact) ?? 'artifact')
    .toLocaleLowerCase()
    .replace(/[^a-z0-9._-]+/g, '-')
    .replace(/^-+|-+$/g, '')
  return `${base || 'artifact'}.depot.json`
}

export function serializeArtifact(artifact: DepotArtifact): string {
  return `${JSON.stringify(artifact, null, 2)}\n`
}
