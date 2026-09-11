import { PackageSearch } from 'lucide-react'

import { artifactKind, type ArtifactType } from './library-model'
import type { DepotArtifact } from '@/lib/api/depot-client'
import { discoverKindPresentation } from './discover-kind-presentation'

export const ARTIFACT_TYPES: ArtifactType[] = ['mcp', 'acp', 'agent', 'skill', 'command', 'plugin', 'marketplace', 'prompt']

const LABELS: Record<string, [string, string]> = {
  mcp: ['MCP', 'MCP'], acp: ['ACP', 'ACP'], agent: ['Agents', 'Agent'],
  skill: ['Skills', 'Skill'], command: ['Commands', 'Command'], plugin: ['Plugins', 'Plugin'],
  marketplace: ['Marketplaces', 'Marketplace'], prompt: ['Prompts', 'Prompt'],
  hook: ['Hooks', 'Hook'], extension: ['Extensions', 'Extension'],
  loadout: ['Loadouts', 'Loadout'], snippet: ['Snippets', 'Snippet'],
}

export function artifactTypeDefinition(kind: string) {
  const key = kind.toLowerCase()
  const [label, singularLabel] = Object.hasOwn(LABELS, key) ? LABELS[key] : [kind || 'Artifact', kind || 'Artifact']
  const presentation = discoverKindPresentation(key)
  // Marketplace is an existing catalog kind outside the mock's taxonomy.
  // Preserve it without reusing a different kind's icon or suggesting support.
  if (key === 'marketplace') {
    const color = 'var(--artifact-marketplace)'
    return { ...presentation, label, singularLabel, color, tone: color, icon: PackageSearch,
      iconStyle: { color, backgroundColor: `color-mix(in srgb, ${color} 10%, transparent)`, borderColor: `color-mix(in srgb, ${color} 30%, transparent)` } }
  }
  return { ...presentation, label, singularLabel }
}

export function ArtifactTypeMark({ artifact, compact = false }: { artifact: DepotArtifact; compact?: boolean }) {
  const kind = artifactKind(artifact)
  const definition = artifactTypeDefinition(kind)
  const Icon = definition.icon
  return <span data-artifact-kind={kind} className="inline-flex shrink-0 items-center gap-1.5 font-bold uppercase tracking-[.07em]" style={{ color: definition.color }}>
    <span className={`${compact ? 'size-[18px] rounded-[5px]' : 'size-8 rounded-[9px]'} grid shrink-0 place-items-center border`} style={definition.iconStyle}>
      <Icon aria-hidden="true" className={compact ? 'size-[13px]' : 'size-4'} />
    </span>
    <span className={compact ? 'text-[9px]' : 'text-[10px]'}>{definition.singularLabel}</span>
  </span>
}
