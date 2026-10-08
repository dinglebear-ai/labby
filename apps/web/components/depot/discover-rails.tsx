'use client'

import Link from 'next/link'
import { Loader2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { DashboardPanel } from '@/components/dashboard/panel'
import type { FederatedArtifact } from '@/lib/api/depot-client'
import { artifactKind, artifactTitle, revisionAge } from './discover-model'
import { DiscoverSourceBadge } from './discover-source-badge'
import { discoverKindPresentation } from './discover-kind-presentation'

type Rail = { label: string; hint: string; empty: string; items: FederatedArtifact[] }

/** Collections over the authorized, retained catalog window; no recommendation evidence is inferred. */
export function discoverReferenceRails(artifacts: FederatedArtifact[], now?: number): Rail[] {
  const dated = artifacts.filter(artifact => {
    const date = Date.parse(artifact.currentRevision?.authoredAt ?? '')
    return Number.isFinite(date) && (now === undefined || date <= now)
  }).sort((a, b) => Date.parse(b.currentRevision!.authoredAt!) - Date.parse(a.currentRevision!.authoredAt!))
  const sources = new Set<string>()
  const sourceSample = artifacts.filter(artifact => {
    if (sources.has(artifact.providerId)) return false
    sources.add(artifact.providerId)
    return true
  })
  return [
    { label: 'Recently updated', hint: 'latest reported revisions', empty: 'No revision dates reported in these results.', items: dated.slice(0, 8) },
    { label: 'From connected sources', hint: 'one artifact per source', empty: 'No source results returned.', items: sourceSample.slice(0, 8) },
    { label: 'Loadouts to explore', hint: 'loadouts in these results', empty: 'No loadouts in these results.', items: artifacts.filter(artifact => artifactKind(artifact) === 'loadout').slice(0, 8) },
  ]
}

export function DiscoverRails({ artifacts, artifactHref, loading = false, incomplete = false, failureMessage, onRetry, now }: {
  artifacts: FederatedArtifact[]
  artifactHref: (providerId?: string, id?: string) => string
  loading?: boolean
  incomplete?: boolean
  failureMessage?: string
  onRetry?: () => void
  now?: number
}) {
  const notice = incomplete ? <div className="flex flex-col items-start gap-3 sm:flex-row sm:items-center sm:justify-between">
    <p role="status" className="w-full min-w-0 text-sm text-aurora-text-muted sm:w-auto sm:flex-1">{failureMessage ?? 'Some catalog sources are unavailable. Results cover the sources that responded.'}</p>
    <div className="flex flex-wrap gap-2">{onRetry ? <Button variant="outline" size="sm" disabled={loading} onClick={onRetry}>{loading ? 'Checking sources…' : 'Retry search'}</Button> : null}<Button asChild variant="ghost" size="sm"><Link href="/settings/depot/">Review sources</Link></Button></div>
  </div> : null

  if (!artifacts.length) return <div data-discover-rails="1" aria-busy={loading}>
    <DashboardPanel title={loading ? 'Loading catalog collections' : incomplete ? 'Catalog results unavailable' : 'No artifacts returned'}>
      {loading ? <p role="status" className="flex items-center gap-2 text-sm text-aurora-text-muted"><Loader2 aria-hidden className="size-4 animate-spin" />Checking connected sources…</p> : incomplete ? notice : <>
        <p role="status" className="text-sm text-aurora-text-muted">Your connected sources returned no artifacts. Publish an artifact or check which sources are enabled.</p>
        <div className="flex flex-wrap gap-2"><Button asChild variant="outline" size="sm"><Link href="/create/">Publish artifact</Link></Button><Button asChild variant="ghost" size="sm"><Link href="/settings/depot/">Review sources</Link></Button></div>
      </>}
    </DashboardPanel>
  </div>

  return <div data-discover-rails="1" aria-busy={loading} className="flex min-w-0 flex-col gap-4">
    <p className="px-0.5 text-xs text-aurora-text-muted">Collections use the currently loaded catalog results.</p>
    {notice}
    {discoverReferenceRails(artifacts, now).filter(rail => rail.items.length > 0).map(rail => <section key={rail.label} aria-label={rail.label} className="flex min-w-0 flex-col gap-2">
      <div className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1 px-0.5">
        <h2 className="font-display text-base font-extrabold text-aurora-text-primary">{rail.label}</h2>
        <span className="text-xs text-aurora-text-muted">{rail.hint}</span>
      </div>
      {rail.items.length ? <div className="aurora-scrollbar flex min-w-0 gap-3 overflow-x-auto px-0.5 pb-2 pt-px">{rail.items.map(artifact => <DiscoverCollectionCard key={`${artifact.providerId}:${artifact.artifactId}`} artifact={artifact} href={artifactHref(artifact.providerId, artifact.artifactId)} now={now} />)}</div> : <p className="rounded-aurora-2 border border-dashed border-aurora-border-strong/60 bg-aurora-panel-medium px-4 py-3 text-sm text-aurora-text-muted">{rail.empty}</p>}
    </section>)}
  </div>
}

function DiscoverCollectionCard({ artifact, href, now }: { artifact: FederatedArtifact; href: string; now?: number }) {
  const kind = artifactKind(artifact)
  const presentation = discoverKindPresentation(kind)
  const Icon = presentation.icon
  const authoredAt = artifact.currentRevision?.authoredAt
  const date = authoredAt && Number.isFinite(Date.parse(authoredAt)) ? authoredAt : undefined
  return <Link href={href} data-discover-collection-artifact={`${artifact.providerId}:${artifact.artifactId}`} className="relative flex w-64 max-w-[calc(100vw-4rem)] shrink-0 flex-col gap-3 rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-strong px-4 py-3 text-left shadow-[var(--aurora-shadow-medium)] transition-colors hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">
    <span className="flex min-w-0 items-center gap-2">
      <span aria-hidden className="grid size-8 shrink-0 place-items-center rounded-aurora-1 border border-aurora-border-default" style={{ color: presentation.color }}><Icon className="size-4" /></span>
      <span className="min-w-0"><span className="block truncate font-display text-sm font-bold text-aurora-text-primary" title={artifactTitle(artifact)}>{artifactTitle(artifact)}</span><span className="text-xs text-aurora-text-muted">{kind}</span></span>
    </span>
    <span className="line-clamp-2 min-h-10 text-sm text-aurora-text-muted">{artifact.description ?? artifact.descriptor?.description ?? 'No description supplied.'}</span>
    <span className="flex min-w-0 flex-wrap items-center justify-between gap-2 border-t border-aurora-border-default pt-2">
      <DiscoverSourceBadge artifact={artifact} />
      {date ? <time dateTime={date} title={`Revision authored ${date}`} className="text-xs text-aurora-text-muted">{revisionAge(date, now)}</time> : null}
    </span>
  </Link>
}
