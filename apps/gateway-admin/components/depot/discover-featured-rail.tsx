'use client'

import { useId } from 'react'
import Link from 'next/link'
import { Download } from 'lucide-react'
import type { FederatedArtifact } from '@/lib/api/depot-client'
import { artifactKey } from '@/lib/depot/provider-model'
import { artifactKind, artifactTitle } from './discover-model'
import { discoverKindPresentation, DiscoverPublisherVerifiedIcon } from './discover-kind-presentation'

export type DiscoverFeaturedItem = {
  artifact: FederatedArtifact
  /** Reported count for this rail's period, never inferred from revisions. */
  installs?: number
}

export type FeaturedState =
  | { state: 'ready'; items: readonly DiscoverFeaturedItem[] }
  | { state: 'loading' }
  | { state: 'unavailable'; message: string }

/** Presentation only. The caller owns authorization, ranking, and time windows. */
export function DiscoverFeaturedRail({ title, subtitle, feed, artifactHref }: {
  title: string
  subtitle: string
  feed: FeaturedState
  artifactHref: (providerId: string, artifactId: string) => string
}) {
  const headingId = useId()
  return <section aria-labelledby={headingId} className="min-w-0 space-y-2">
    <div className="flex flex-wrap items-baseline gap-x-[9px] gap-y-1 px-0.5">
      <h2 id={headingId} className="font-display text-base font-extrabold tracking-[-0.012em] text-aurora-text-primary">{title}</h2>
      <p className="text-[11.5px] text-aurora-text-muted">{subtitle}</p>
    </div>
    {feed.state === 'loading' ? <div role="status" className="rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-medium p-4 text-xs text-aurora-text-muted">Loading {title.toLowerCase()}…</div>
      : feed.state === 'unavailable' ? <p className="rounded-aurora-2 border border-dashed border-aurora-border-subtle p-4 text-xs text-aurora-text-muted">{feed.message}</p>
      : feed.items.length === 0 ? <p className="rounded-aurora-2 border border-dashed border-aurora-border-subtle p-4 text-xs text-aurora-text-muted">No artifacts in this feed yet.</p>
      : <ul aria-label={title} className="aurora-scrollbar flex snap-x snap-proximity gap-[11px] overflow-x-auto overflow-y-hidden px-0.5 pt-px pb-[7px]">
        {feed.items.map(({ artifact, installs }) => {
          const kind = artifactKind(artifact)
          const { icon: Icon, color, tone, iconStyle } = discoverKindPresentation(kind)
          const validInstalls = Number.isSafeInteger(installs) && installs! >= 0
          return <li key={artifactKey(artifact.providerId, artifact.artifactId)} className="w-[264px] max-w-[88%] shrink-0 snap-start">
            <Link href={artifactHref(artifact.providerId, artifact.artifactId)}
              style={{ background: 'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))' }}
              className="relative flex h-full flex-col gap-[9px] rounded-aurora-2 border border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))] px-[15px] py-[13px] shadow-aurora-medium transition-colors hover:border-aurora-border-strong focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-aurora-accent-primary">
              <span aria-hidden="true" className="pointer-events-none absolute inset-y-3.5 left-0 w-0.5" style={{ background: `color-mix(in srgb, ${tone} 60%, transparent)` }} />
              <div className="flex min-w-0 items-center gap-[9px]">
                <span aria-hidden="true" style={iconStyle} className="grid size-[34px] shrink-0 place-items-center rounded-[10px] border"><Icon className="size-[18px]" /></span>
                <div className="min-w-0"><div className="flex min-w-0 items-center gap-[5px]"><h3 className="truncate font-display text-sm font-[760] text-aurora-text-primary" title={artifactTitle(artifact)}>{artifactTitle(artifact)}</h3>
                  {artifact.publisherVerified === true ? <DiscoverPublisherVerifiedIcon aria-label="Publisher verified" className="size-[11px] shrink-0 text-aurora-success" /> : null}</div>
                  <p style={{ color }} className="mt-0.5 text-[10.5px] font-[650] uppercase tracking-[0.06em]">{kind}</p>
                </div>
              </div>
              <p className="line-clamp-2 flex-1 text-pretty text-[11.5px] leading-[1.5] text-aurora-text-muted">{artifact.description ?? artifact.descriptor?.description ?? 'No description supplied by this source.'}</p>
              <div className="flex min-w-0 items-center justify-between gap-[7px] border-t border-[color-mix(in_srgb,var(--aurora-border-default)_45%,transparent)] pt-2 text-[10.5px] text-aurora-text-muted">
                <span className="truncate">{artifact.namespace ?? artifact.descriptor?.namespace ?? 'Publisher not supplied'}</span>
                {validInstalls ? <span aria-label={`${installs} installs`} className="flex shrink-0 items-center gap-1 whitespace-nowrap font-[650] tabular-nums text-[color-mix(in_srgb,var(--aurora-text-muted)_85%,var(--aurora-text-primary))]"><Download aria-hidden="true" className="size-[11px]" />{new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 }).format(installs!)}</span> : null}
              </div>
            </Link>
          </li>
        })}
      </ul>}
  </section>
}
