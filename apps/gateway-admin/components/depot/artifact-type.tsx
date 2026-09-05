import type { ComponentType } from 'react'
import { Bot, Boxes, Braces, Command, MessagesSquare, PackageSearch, PlugZap, Sparkles } from 'lucide-react'

import { artifactKind, type ArtifactType } from './library-model'
import type { DepotArtifact } from '@/lib/api/depot-client'

type TypeDefinition = { label: string; color: string; icon: ComponentType<{ className?: string }> }

export const ARTIFACT_TYPES: ArtifactType[] = ['mcp', 'acp', 'agent', 'skill', 'command', 'plugin', 'marketplace', 'prompt']

const TYPE_DEFINITIONS: Record<ArtifactType, TypeDefinition> = {
  mcp: { label: 'MCP', color: 'var(--artifact-mcp)', icon: Boxes },
  acp: { label: 'ACP', color: 'var(--artifact-acp)', icon: Braces },
  agent: { label: 'Agents', color: 'var(--artifact-agent)', icon: Bot },
  skill: { label: 'Skills', color: 'var(--artifact-skill)', icon: Sparkles },
  command: { label: 'Commands', color: 'var(--artifact-command)', icon: Command },
  plugin: { label: 'Plugins', color: 'var(--artifact-plugin)', icon: PlugZap },
  marketplace: { label: 'Marketplaces', color: 'var(--artifact-marketplace)', icon: PackageSearch },
  prompt: { label: 'Prompts', color: 'var(--artifact-prompt)', icon: MessagesSquare },
}

export function artifactTypeDefinition(kind: string): TypeDefinition {
  return TYPE_DEFINITIONS[kind as ArtifactType] ?? { label: kind || 'Artifact', color: 'var(--aurora-text-muted)', icon: Boxes }
}

export function ArtifactTypeMark({ artifact, compact = false }: { artifact: DepotArtifact; compact?: boolean }) {
  const kind = artifactKind(artifact)
  const definition = artifactTypeDefinition(kind)
  const Icon = definition.icon
  return <span className="inline-flex shrink-0 items-center gap-2 font-bold uppercase tracking-[.12em]" style={{ color: definition.color }}>
    <span className={`${compact ? 'size-7' : 'size-9'} grid place-items-center rounded-aurora-1 border bg-[color-mix(in_srgb,currentColor_10%,transparent)]`} style={{ borderColor: `color-mix(in srgb, ${definition.color} 38%, transparent)` }}>
      <Icon className={compact ? 'size-3.5' : 'size-4'} />
    </span>
    <span className="text-[10px]">{definition.label}</span>
  </span>
}
