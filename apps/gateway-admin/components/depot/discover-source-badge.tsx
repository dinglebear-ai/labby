import { Badge } from '@/components/ui/badge'
import type { ArtifactSourceOrigin, FederatedArtifact } from '@/lib/api/depot-client'

type SourcePresentation = { label: string; color: string }

// Every origin the client schema accepts has a presentation, so a newly accepted
// origin cannot silently fall back to the raw provider id.
const sourcePresentations = {
  'mcp-registry': { label: 'MCP Registry', color: 'var(--aurora-accent-primary)' },
  'acp-registry': { label: 'ACP Registry', color: 'var(--aurora-warn)' },
  ard: { label: 'ARD', color: 'var(--aurora-success)' },
  'skills-sh': { label: 'skills.sh', color: 'var(--aurora-accent-strong)' },
  github: { label: 'GitHub', color: 'var(--aurora-text-muted)' },
  claude: { label: 'Claude', color: 'var(--aurora-accent-pink)' },
  gemini: { label: 'Gemini', color: 'var(--aurora-accent-primary)' },
  'agent-plugins': { label: 'Agent Plugins', color: 'var(--aurora-warn)' },
  'web-crawl': { label: 'Web Crawl', color: 'var(--aurora-accent-pink-strong)' },
} satisfies Record<ArtifactSourceOrigin, SourcePresentation>

/** Origin classification is supplied by Depot, never guessed from a provider name. */
export function DiscoverSourceBadge({ artifact }: { artifact: Pick<FederatedArtifact, 'providerId' | 'sourceOrigin'> }) {
  const source: SourcePresentation | undefined = artifact.sourceOrigin ? sourcePresentations[artifact.sourceOrigin] : undefined
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
