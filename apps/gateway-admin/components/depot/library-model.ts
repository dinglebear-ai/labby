import type { DepotArtifact } from '@/lib/api/depot-client'

export type LibraryKind = 'all' | string
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
  return KIND_ALIASES[kind] ?? kind
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
