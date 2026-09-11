'use client'

import Link from 'next/link'
import { Download, GitFork, Star } from 'lucide-react'
import { AURORA_BADGE_LABEL, AURORA_CARD_TITLE, AURORA_DENSE_META } from '@/components/aurora/tokens'
import { Badge } from '@/components/ui/badge'

import { DiscoverFileCount } from './discover-file-count'
import { DiscoverFormatMark } from './discover-format-mark'
import { DiscoverSourceBadge } from './discover-source-badge'
import type { FederatedArtifact } from '@/lib/api/depot-client'
import { artifactKey } from '@/lib/depot/provider-model'
import { artifactKind, artifactTitle, revisionAge } from './discover-model'
import type { DiscoveryDensity } from './discover-view-options'
import { discoverKindPresentation, DiscoverPublisherVerifiedIcon } from './discover-kind-presentation'

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
  const { family, icon: Icon, color, tone, iconStyle } = discoverKindPresentation(kind)
  const namespace = artifact.namespace ?? artifact.descriptor?.namespace
  const description = artifact.description ?? artifact.descriptor?.description
  const revisionDate = artifact.currentRevision?.authoredAt
  const validDate = revisionDate && Number.isFinite(Date.parse(revisionDate)) ? revisionDate : undefined
  const metrics = [
    { key: 'stars', label: 'Stars', icon: Star, color: 'var(--aurora-warn)' },
    { key: 'installs', label: 'Installs', icon: Download, color: 'var(--aurora-accent-strong)' },
    { key: 'forks', label: 'Forks', icon: GitFork, color: 'var(--aurora-accent-pink)' },
  ] as const
  const reportedMetrics = metrics.filter(metric => {
    const value = artifact.metrics?.[metric.key]
    return Number.isSafeInteger(value) && value! >= 0
  })

  const spacing = density === 'compact' ? 'gap-[9px] px-3.5 pt-2.5 pb-[9px]' : density === 'comfortable' ? 'gap-3 p-5' : 'gap-[9px] px-4 pt-3.5 pb-[13px]'
  return <article data-density={density} style={{ background: 'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))' }}
    className={`@container/discover-card relative flex min-w-0 flex-col rounded-aurora-2 border ${selected ? 'border-aurora-accent-primary' : 'border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))]'} bg-aurora-panel-medium shadow-[var(--aurora-shadow-medium)]`}>
    <span aria-hidden="true" data-kind-stripe className="pointer-events-none absolute bottom-3.5 left-0 top-3.5 w-0.5" style={{ background: `color-mix(in srgb, ${tone} 60%, transparent)` }} />
    <Link href={href} data-artifact-key={artifactKey(artifact.providerId, artifact.artifactId)} aria-current={selected ? 'page' : undefined}
      className={`group flex min-w-0 flex-1 flex-col ${spacing} rounded-aurora-2 transition-colors hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary ${compact ? '@min-[640px]/discover-card:grid @min-[640px]/discover-card:grid-cols-[minmax(0,1fr)_minmax(0,2fr)] @min-[640px]/discover-card:items-center' : ''}`}>
      <div className="min-w-0 space-y-[9px]">
        <div className="flex flex-wrap items-center justify-between gap-[var(--space-2)]">
          <span className={`flex items-center gap-2 ${AURORA_BADGE_LABEL} text-[9.5px]`} style={{ color }}>
            <span aria-hidden="true" style={iconStyle} className="grid size-8 shrink-0 place-items-center rounded-[9px] border"><Icon className="size-4" /></span>
            <span className="flex flex-wrap items-center gap-1.5">{kind}
              {family ? <span className="text-[9px] tracking-widest text-aurora-text-muted">{family}</span> : null}
            </span>
          </span>
          <DiscoverSourceBadge artifact={artifact} />
        </div>
        <div className="min-w-0">
          <div className="flex min-w-0 items-center gap-1.5"><h3 className={`${AURORA_CARD_TITLE} truncate text-[15px] font-[760] tracking-[-0.01em] text-aurora-text-primary`} title={artifactTitle(artifact)}>{artifactTitle(artifact)}</h3>
            {artifact.publisherVerified === true ? <DiscoverPublisherVerifiedIcon aria-label="Publisher verified" className="size-3 shrink-0 text-aurora-success" /> : null}
          </div>
          <div className={`mt-[3px] flex flex-wrap items-center gap-[7px] ${AURORA_DENSE_META} text-aurora-text-muted`}>
            <span className="min-w-0 truncate">{namespace || 'Publisher not supplied'}</span>
            {artifact.publisherVerified === true ? <span title="Publisher verification reported by source" className="inline-flex h-4 shrink-0 items-center rounded border border-aurora-success/35 bg-aurora-success/10 px-1.5 text-[8.5px] font-bold uppercase tracking-[0.08em] text-aurora-success">Verified</span> : null}
            <span className="flex-1" />
            {validDate ? <time className="inline-flex shrink-0 items-center gap-[5px] text-[10.5px] font-semibold" dateTime={validDate} title={`Revision authored ${validDate}`}><span aria-hidden="true" className="size-1 rounded-full bg-current" />{revisionAge(validDate, now)}</time> : null}
          </div>
        </div>
      </div>
      <div className="min-w-0 flex-1 space-y-[var(--space-2)]">
        <p className="line-clamp-2 text-pretty text-xs leading-normal text-aurora-text-muted">
          {description || 'No description supplied by this source.'}
        </p>
        <div style={{ '--discover-kind-tone': color } as React.CSSProperties} className="flex flex-wrap gap-[5px] [&>[data-slot=badge]]:h-[17px] [&>[data-slot=badge]]:rounded [&>[data-slot=badge]]:border-[color-mix(in_srgb,var(--discover-kind-tone)_26%,transparent)] [&>[data-slot=badge]]:bg-[color-mix(in_srgb,var(--discover-kind-tone)_8%,transparent)] [&>[data-slot=badge]]:px-[7px] [&>[data-slot=badge]]:py-0 [&>[data-slot=badge]]:text-[9.5px] [&>[data-slot=badge]]:font-semibold">
          <DiscoverFileCount count={artifact.currentRevision?.fileCount} />
          {artifact.provenance?.originalFormat ? <Badge variant="outline" title="Source format" className="max-w-full gap-1.5 truncate"><DiscoverFormatMark format={artifact.provenance.originalFormat} />{artifact.provenance.originalFormat}</Badge> : null}
        </div>
      </div>
    </Link>
    {reportedMetrics.length ? <div className="mx-4 flex flex-wrap items-center gap-x-2.5 gap-y-1 border-t border-[color-mix(in_srgb,var(--aurora-border-default)_45%,transparent)] pb-[13px] pt-2">
      {reportedMetrics.map(({ key, label, icon: MetricIcon, color: metricColor }) => <span key={key} title={label} aria-label={`${artifact.metrics![key]} ${label.toLowerCase()}`} className="inline-flex items-center gap-1 text-[10.5px] font-[650] tabular-nums text-aurora-text-muted"><MetricIcon aria-hidden="true" className="size-[11px]" style={{ color: metricColor }} />{new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 }).format(artifact.metrics![key]!)}</span>)}
    </div> : null}
  </article>
}
