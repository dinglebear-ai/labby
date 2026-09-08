'use client'

import Link from 'next/link'
import { ArrowUpRight, Blocks, Bot, Braces, Cable, FileCode2, Layers3, MessageSquare, Shield, Terminal, type LucideIcon } from 'lucide-react'
import { AURORA_BADGE_LABEL, AURORA_CARD_TITLE, AURORA_DENSE_META } from '@/components/aurora/tokens'
import { Badge } from '@/components/ui/badge'
import type { FederatedArtifact } from '@/lib/api/depot-client'
import { artifactKey } from '@/lib/depot/provider-model'
import { artifactKind, artifactTitle, revisionAge } from './discover-model'
import type { DiscoveryDensity } from './discover-view-options'

const kindPresentation: Record<string, { family: string; icon: LucideIcon }> = {
  mcp: { family: 'Protocol', icon: Cable },
  acp: { family: 'Protocol', icon: Cable },
  skill: { family: 'Capability', icon: FileCode2 },
  command: { family: 'Capability', icon: Terminal },
  snippet: { family: 'Capability', icon: Braces },
  agent: { family: 'Authored', icon: Bot },
  prompt: { family: 'Authored', icon: MessageSquare },
  plugin: { family: 'Bundle', icon: Blocks },
  extension: { family: 'Bundle', icon: Blocks },
  loadout: { family: 'Bundle', icon: Layers3 },
  hook: { family: 'Guard', icon: Shield },
}

/** Source identity remains explicit even when two providers return the same artifact. */
export function DiscoverArtifactCard({ artifact, compact, selected, href, now, density = compact ? 'compact' : 'default' }: {
  artifact: FederatedArtifact
  compact: boolean
  selected: boolean
  href: string
  density?: DiscoveryDensity
  now?: number
}) {
  const kind = artifactKind(artifact)
  const { family, icon: Icon } = kindPresentation[kind.toLowerCase()] ?? { family: 'Artifact', icon: Layers3 }
  const namespace = artifact.namespace ?? artifact.descriptor?.namespace
  const description = artifact.description ?? artifact.descriptor?.description
  const revisionDate = artifact.currentRevision?.authoredAt
  const validDate = revisionDate && Number.isFinite(Date.parse(revisionDate)) ? revisionDate : undefined

  const spacing = density === 'compact' ? 'gap-2 p-3' : density === 'comfortable' ? 'gap-4 p-5' : 'gap-3 p-4'
  return <article data-density={density}
    className={`min-w-0 rounded-aurora-2 border ${selected ? 'border-aurora-accent-primary' : 'border-aurora-border-default'} bg-aurora-panel-medium shadow-[var(--aurora-shadow-subtle)]`}>
    <Link href={href} data-artifact-key={artifactKey(artifact.providerId, artifact.artifactId)} aria-current={selected ? 'page' : undefined}
      className={`group flex h-full min-w-0 flex-col ${spacing} rounded-aurora-2 transition-colors hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary ${compact ? 'md:grid md:grid-cols-[minmax(0,1fr)_minmax(0,2fr)_auto] md:items-center' : ''}`}>
      <div className="min-w-0 space-y-[var(--space-4)]">
        <div className="flex flex-wrap items-center justify-between gap-[var(--space-2)]">
          <span className={`flex items-center gap-[var(--space-2)] ${AURORA_BADGE_LABEL} text-aurora-accent-primary`}>
            <Icon aria-hidden="true" className="size-4 shrink-0" />{kind}
            <span className="text-aurora-text-muted">{family}</span>
          </span>
          <Badge variant="outline" className="max-w-full truncate" title={artifact.providerId}>{artifact.providerId}</Badge>
        </div>
        <div className="min-w-0">
          <h3 className={`${AURORA_CARD_TITLE} truncate text-aurora-text-primary`} title={artifactTitle(artifact)}>{artifactTitle(artifact)}</h3>
          <div className={`mt-[var(--space-2)] flex flex-wrap items-center gap-[var(--space-2)] ${AURORA_DENSE_META} text-aurora-text-muted`}>
            <span className="truncate">{namespace || 'Publisher not supplied'}</span>
            {validDate ? <time dateTime={validDate} title={`Revision authored ${validDate}`}>{revisionAge(validDate, now)}</time> : null}
          </div>
        </div>
      </div>
      <div className="min-w-0 flex-1 space-y-[var(--space-4)]">
        <p className={`line-clamp-2 text-xs leading-[var(--lh-body)] text-aurora-text-muted ${compact ? '' : 'min-h-10'}`}>
          {description || 'No description supplied by this source.'}
        </p>
        <div className="flex flex-wrap gap-[var(--space-2)]">
          {artifact.publication?.visibility ? <Badge variant="outline">{artifact.publication.visibility}</Badge> : null}
          {artifact.publication?.distribution ? <Badge variant="outline">{artifact.publication.distribution}</Badge> : null}
          {artifact.revisionCount !== undefined ? <Badge variant="outline">{artifact.revisionCount} revisions</Badge> : null}
        </div>
      </div>
      <div className={`flex min-w-0 items-center justify-between gap-[var(--space-3)] ${AURORA_DENSE_META} text-aurora-text-muted ${compact ? '' : 'mt-auto border-t border-aurora-border-default pt-[var(--space-4)]'}`}>
        <span className="truncate" title={artifact.artifactId}>{artifact.artifactId}</span>
        <span className="flex shrink-0 items-center gap-[var(--space-1)] text-aurora-accent-primary">Inspect <ArrowUpRight aria-hidden="true" className="size-3.5" /></span>
      </div>
    </Link>
  </article>
}
