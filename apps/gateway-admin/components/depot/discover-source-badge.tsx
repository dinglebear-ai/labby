import { Activity, CircleCheck, Server } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import type { FederatedArtifact } from '@/lib/api/depot-client'

export const DISCOVER_SOURCE_ORIGINS = {
  'mcp-registry': { label: 'MCP Registry', icon: Server, color: 'var(--aurora-accent-primary)' },
  'acp-registry': { label: 'ACP Registry', icon: Activity, color: 'var(--aurora-warn)' },
  ard: { label: 'ARD', icon: CircleCheck, color: 'var(--aurora-success)' },
} as const

// Display evidence is independent of the three currently supported filters.
const sourcePresentations = {
  ...DISCOVER_SOURCE_ORIGINS,
  'skills-sh': { label: 'skills.sh', color: 'var(--aurora-accent-strong)' },
  github: { label: 'GitHub', color: 'var(--aurora-text-muted)' },
  claude: { label: 'Claude', color: 'var(--aurora-accent-pink)' },
  gemini: { label: 'Gemini', color: 'var(--aurora-accent-primary)' },
  'agent-plugins': { label: 'Agent Plugins', color: 'var(--aurora-warn)' },
  'web-crawl': { label: 'Web Crawl', color: 'var(--aurora-accent-pink-strong)' },
} as const

/** Origin classification is supplied by Depot, never guessed from a provider name. */
export function DiscoverSourceBadge({ artifact }: { artifact: Pick<FederatedArtifact, 'providerId' | 'sourceOrigin'> }) {
  const source = artifact.sourceOrigin ? sourcePresentations[artifact.sourceOrigin] : undefined
  const label = source?.label ?? artifact.providerId
  const color = source?.color ?? 'var(--aurora-text-muted)'
  return <Badge variant="outline" className="h-[18px] max-w-full gap-1 rounded-[4px] px-[7px] text-[9px] font-[650] uppercase tracking-[0.04em]"
    style={{ color, background: `color-mix(in srgb, ${color} 8%, transparent)`, borderColor: `color-mix(in srgb, ${color} 22%, transparent)` }}
    title={source ? `${source.label} · via ${artifact.providerId}` : artifact.providerId}>
    <span aria-hidden="true" data-source-dot="1" className="size-1 shrink-0 rounded-full bg-current" />
    <span className="truncate">{label}</span>
    {source ? <span className="sr-only">via {artifact.providerId}</span> : null}
  </Badge>
}
