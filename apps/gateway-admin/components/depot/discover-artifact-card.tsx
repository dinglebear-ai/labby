'use client'

import { useRef } from 'react'
import Link from 'next/link'
import { Braces, Check, Clock3, Download, FileText, GitFork, List, Loader2, Package, Plus, Search, Send, ShieldCheck, Star, Terminal, Wrench } from 'lucide-react'
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
export function DiscoverArtifactCard({ artifact, compact, selected, href, now, density = compact ? 'compact' : 'comfortable', selectionMode = false, selectedForBulk = false, cursorActive = false, matchLabel, specLabels, metricLabels, onToggleSelected, onEnterSelectionMode, onFork, onSend, onAdd, inLibrary = false, actionPending = false }: {
  artifact: FederatedArtifact
  compact: boolean
  selected: boolean
  href: string
  density?: DiscoveryDensity
  now?: number
  selectionMode?: boolean
  selectedForBulk?: boolean
  cursorActive?: boolean
  matchLabel?: string
  specLabels?: readonly string[]
  metricLabels?: Partial<Record<'stars' | 'installs' | 'forks', string>>
  onToggleSelected?: () => void
  onEnterSelectionMode?: () => void
  onFork?: () => void | Promise<void>
  onSend?: () => void | Promise<void>
  onAdd?: () => void | Promise<void>
  inLibrary?: boolean
  actionPending?: boolean
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
  const pressTimer = useRef<number | undefined>(undefined)
  const longPressFired = useRef(false)
  const selectionVisible = selectionMode || selectedForBulk
  const clearPress = () => { if (pressTimer.current !== undefined) window.clearTimeout(pressTimer.current); pressTimer.current = undefined }
  const beginPress = () => {
    clearPress()
    longPressFired.current = false
    if (!onEnterSelectionMode || selectionMode) return
    pressTimer.current = window.setTimeout(() => { longPressFired.current = true; onEnterSelectionMode() }, 420)
  }
  const reportedMetrics = metrics.filter(metric => {
    const value = artifact.metrics?.[metric.key]
    return Number.isSafeInteger(value) && value! >= 0
  })

  if (compact) {
    const installs = Number.isSafeInteger(artifact.metrics?.installs) && artifact.metrics!.installs! >= 0 ? artifact.metrics!.installs : undefined
    return <article data-density={density} data-discover-result={artifactKey(artifact.providerId, artifact.artifactId)} data-cursor-active={cursorActive ? 'true' : undefined} style={{ borderLeftColor: `color-mix(in srgb, ${tone} 45%, transparent)` }} className={`relative -mx-[7px] -my-1 box-border flex min-w-0 items-center gap-2.5 border-t border-l-2 border-t-[color-mix(in_srgb,var(--aurora-border-default)_40%,var(--aurora-page-bg))] px-4 ${density === 'compact' ? 'py-1.5' : 'py-2.5'} transition-colors hover:bg-aurora-hover-bg focus-within:ring-1 focus-within:ring-inset focus-within:ring-aurora-accent-primary ${selected ? 'ring-1 ring-inset ring-aurora-accent-primary' : cursorActive ? 'ring-1 ring-inset ring-aurora-accent-pink' : ''}`}>
      {selectionVisible ? <button type="button" aria-pressed={selectedForBulk} aria-label={`Select ${artifactTitle(artifact)}`} title="Select for bulk actions" onClick={event => { event.stopPropagation(); onToggleSelected?.() }} className={`grid size-[15px] shrink-0 place-items-center rounded-[5px] border p-0 ${selectedForBulk ? 'border-[color-mix(in_srgb,var(--aurora-accent-primary)_70%,transparent)] bg-aurora-accent-primary text-[#06202e]' : 'border-[color-mix(in_srgb,var(--aurora-border-strong)_70%,transparent)] bg-[var(--gw0-0_45)] text-transparent'}`}>{selectedForBulk ? <Check aria-hidden className="size-2.5" strokeWidth={3.2}/> : null}</button> : null}
      <Link href={href} data-artifact-key={artifactKey(artifact.providerId, artifact.artifactId)} aria-current={selected ? 'page' : undefined} onMouseDown={beginPress} onMouseUp={clearPress} onMouseLeave={clearPress} onTouchStart={beginPress} onTouchEnd={clearPress} onClick={event => { if (selectionMode || longPressFired.current) { event.preventDefault(); longPressFired.current = false; onToggleSelected?.() } }} className="contents">
        <span aria-hidden="true" style={iconStyle} className="grid size-6 shrink-0 place-items-center rounded-[7px] border"><Icon className="size-3" /></span>
        <span className="flex w-[78px] shrink-0 flex-col gap-px"><span className="text-[9.5px] font-bold uppercase leading-3 tracking-[0.09em]" style={{ color }}>{kind}</span>{family ? <span className="text-[8.5px] font-semibold uppercase leading-[9px] tracking-[0.08em] text-aurora-text-muted/70">{family}</span> : null}</span>
        <span className="min-w-[96px] flex-1 truncate pr-0.5 text-[13px] font-semibold leading-4 text-aurora-text-primary">{artifactTitle(artifact)}</span>
        <span className="hidden min-w-[96px] flex-1 truncate pr-0.5 text-[11.5px] leading-[14px] text-aurora-text-muted sm:block">{description || 'No description supplied by this source.'}</span>
        <span className="hidden box-content w-[120px] shrink-0 truncate pr-0.5 text-[11px] leading-[14px] text-aurora-text-muted min-[1401px]:block">{namespace || 'Publisher not supplied'}</span>
        <span className="hidden h-3 w-24 shrink-0 items-center leading-3 min-[1181px]:flex"><DiscoverSourceBadge artifact={artifact} plain/></span>
        <span className="sr-only"><DiscoverFileCount count={artifact.currentRevision?.fileCount}/>{artifact.provenance?.originalFormat ? <Badge variant="outline" title="Source format"><DiscoverFormatMark format={artifact.provenance.originalFormat}/>{artifact.provenance.originalFormat}</Badge> : null}</span>
        {installs !== undefined ? <span title="Installs" aria-label={`${installs} installs`} className="hidden w-[62px] shrink-0 items-center justify-end gap-1 text-[11px] font-semibold leading-[14px] tabular-nums text-aurora-text-muted md:inline-flex"><Download aria-hidden className="size-[11px] text-aurora-accent-strong"/>{metricLabels?.installs ?? new Intl.NumberFormat('en',{notation:'compact',maximumFractionDigits:1}).format(installs)}</span> : <span className="hidden w-[62px] shrink-0 md:block"/>}
      </Link>
      {onFork || onSend || onAdd ? <span data-discover-actions className="inline-flex shrink-0 items-center gap-1">
        {onFork ? <button type="button" aria-label="Fork" title="Fork into your Library" disabled={actionPending} onClick={() => void onFork()} className="grid size-[26px] place-items-center rounded-lg border border-aurora-border-strong/55 bg-aurora-control-surface text-aurora-text-muted hover:text-aurora-accent-pink disabled:opacity-50"><GitFork aria-hidden className="size-[13px]"/></button> : null}
        {onSend ? <button type="button" aria-label="Send to Labby" title="Send to your Labby instance" disabled={actionPending} onClick={() => void onSend()} className="grid size-[26px] place-items-center rounded-lg border border-aurora-border-strong/55 bg-aurora-control-surface text-aurora-text-muted hover:text-aurora-accent-strong disabled:opacity-50"><Send aria-hidden className="size-[13px]"/></button> : null}
        {onAdd ? <button type="button" aria-label={inLibrary ? 'Already in your Library' : 'Add to Library'} title={inLibrary ? 'Already in your Library' : 'Add to Library'} disabled={actionPending || inLibrary} onClick={() => void onAdd()} className={`grid size-[26px] place-items-center rounded-lg border ${inLibrary ? 'border-aurora-success/40 bg-aurora-success/10 text-aurora-success' : 'border-aurora-accent-primary/50 bg-aurora-accent-primary/10 text-[#bfe7fb]'} disabled:opacity-70`}>{actionPending ? <Loader2 aria-hidden className="size-[13px] animate-spin"/> : inLibrary ? <Check aria-hidden className="size-[13px]"/> : <Plus aria-hidden className="size-[13px]"/>}</button> : null}
      </span> : null}
    </article>
  }

  const spacing = density === 'compact' ? 'gap-1 px-[14px] pb-[9px] pt-[10px]' : specLabels?.length ? 'gap-[9px] px-4 pb-[10px] pt-3.5' : 'gap-[9px] px-4 pt-3.5 pb-[13px]'
  return <article data-density={density} data-discover-result={artifactKey(artifact.providerId, artifact.artifactId)} data-cursor-active={cursorActive ? 'true' : undefined} style={{ background: 'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))' }}
    className={`@container/discover-card relative flex min-w-0 flex-col rounded-aurora-2 border ${selected ? 'border-aurora-accent-primary' : cursorActive ? 'border-[color-mix(in_srgb,var(--aurora-accent-pink-deep)_70%,var(--aurora-border-strong))] shadow-[var(--aurora-shadow-medium),0_0_0_1px_color-mix(in_srgb,var(--aurora-accent-pink-deep)_55%,transparent)]' : 'border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))]'} bg-aurora-panel-medium shadow-[var(--aurora-shadow-medium)]`}>
    <span aria-hidden="true" data-kind-stripe className="pointer-events-none absolute bottom-3.5 left-0 top-3.5 w-0.5" style={{ background: `color-mix(in srgb, ${tone} 60%, transparent)` }} />
    {selectionVisible ? <button type="button" aria-pressed={selectedForBulk} aria-label={`Select ${artifactTitle(artifact)}`} title="Select for bulk actions" onClick={event => { event.stopPropagation(); onToggleSelected?.() }} className={`${compact ? 'left-[14px] top-[11px] size-[15px]' : 'left-4 top-[18px] size-4'} absolute z-10 grid place-items-center rounded-[5px] border p-0 ${selectedForBulk ? 'border-[color-mix(in_srgb,var(--aurora-accent-primary)_70%,transparent)] bg-aurora-accent-primary text-[#06202e]' : 'border-[color-mix(in_srgb,var(--aurora-border-strong)_70%,transparent)] bg-[var(--gw0-0_45)] text-transparent'}`}>{selectedForBulk ? <Check aria-hidden className="size-2.5" strokeWidth={3.2}/> : null}</button> : null}
    <Link href={href} data-artifact-key={artifactKey(artifact.providerId, artifact.artifactId)} aria-current={selected ? 'page' : undefined}
      onMouseDown={beginPress} onMouseUp={clearPress} onMouseLeave={clearPress} onTouchStart={beginPress} onTouchEnd={clearPress}
      onClick={event => { if (selectionMode || longPressFired.current) { event.preventDefault(); longPressFired.current = false; onToggleSelected?.() } }}
      className={`group flex min-w-0 flex-1 flex-col ${spacing} rounded-aurora-2 transition-colors hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary ${compact ? '@min-[640px]/discover-card:grid @min-[640px]/discover-card:grid-cols-[minmax(0,1fr)_minmax(0,2fr)] @min-[640px]/discover-card:items-center' : ''}`}>
      <div className="min-w-0 space-y-[9px]">
        <div className={`flex flex-wrap items-center justify-between gap-[var(--space-2)] ${selectionVisible ? 'pl-6' : ''}`}>
          <span className={`flex items-center gap-2 ${AURORA_BADGE_LABEL} text-[9.5px]`} style={{ color }}>
            <span aria-hidden="true" style={iconStyle} className="grid size-[34px] shrink-0 place-items-center rounded-[9px] border"><Icon className="size-4" /></span>
            <span className="flex flex-wrap items-center gap-1.5">{kind}
              {family ? <span className="text-[9px] tracking-widest text-aurora-text-muted">{family}</span> : null}
            </span>
          </span>
          <DiscoverSourceBadge artifact={artifact} />
        </div>
        <div className="min-w-0">
          <div className="flex h-[21px] min-w-0 items-center gap-1.5"><h3 className={`${AURORA_CARD_TITLE} truncate text-[15px] font-[760] leading-[21px] tracking-[-0.01em] text-aurora-text-primary`} title={artifactTitle(artifact)}>{artifactTitle(artifact)}</h3>
            {artifact.publisherVerified === true ? <DiscoverPublisherVerifiedIcon aria-label="Publisher verified" className="size-3 shrink-0 text-aurora-success" /> : null}
          </div>
          <div className={`mt-[3px] flex h-[18px] flex-wrap items-center gap-[7px] ${AURORA_DENSE_META} leading-[18px] text-aurora-text-muted`}>
            <span className="min-w-0 truncate">{namespace || 'Publisher not supplied'}</span>
            {artifact.publisherVerified === true ? <span title="Publisher verification reported by source" className="inline-flex h-4 shrink-0 items-center rounded border border-aurora-success/35 bg-aurora-success/10 px-1.5 text-[8.5px] font-bold uppercase tracking-[0.08em] text-aurora-success">Verified</span> : null}
            <span className="flex-1" />
            {validDate ? <time className="inline-flex shrink-0 items-center gap-[5px] text-[10.5px] font-semibold" dateTime={validDate} title={`Revision authored ${validDate}`}><span aria-hidden="true" className="size-1 rounded-full bg-current" />{revisionAge(validDate, now)}</time> : null}
          </div>
        </div>
      </div>
      {matchLabel ? <span data-match-label className="inline-flex items-center gap-[5px] text-[10px] font-[650] leading-3 tracking-[0.04em] text-aurora-accent-pink" style={{ marginTop: -3 }}><Search aria-hidden className="size-2.5" strokeWidth={1.8}/>{matchLabel}</span> : null}
      <div className={`min-w-0 flex-1 ${specLabels?.length ? 'space-y-[9px]' : 'space-y-[var(--space-2)]'}`}>
        <p className="line-clamp-2 text-pretty text-xs leading-normal text-aurora-text-muted">
          {description || 'No description supplied by this source.'}
        </p>
        <div style={{ '--discover-kind-tone': color } as React.CSSProperties} className={specLabels?.length ? "flex min-h-[19px] flex-wrap items-center gap-[5px]" : "flex flex-wrap gap-[5px] [&>[data-slot=badge]]:h-[17px] [&>[data-slot=badge]]:rounded [&>[data-slot=badge]]:border-[color-mix(in_srgb,var(--discover-kind-tone)_26%,transparent)] [&>[data-slot=badge]]:bg-[color-mix(in_srgb,var(--discover-kind-tone)_8%,transparent)] [&>[data-slot=badge]]:px-[7px] [&>[data-slot=badge]]:py-0 [&>[data-slot=badge]]:text-[9.5px] [&>[data-slot=badge]]:font-semibold"}>
          {specLabels?.length ? specLabels.map(label => <span key={label} className="inline-flex h-[19px] max-w-full items-center gap-[5px] truncate rounded-md border border-[color-mix(in_srgb,var(--discover-kind-tone)_26%,transparent)] bg-[color-mix(in_srgb,var(--discover-kind-tone)_8%,transparent)] px-[7px] text-[9.5px] font-semibold text-aurora-text-muted"><DiscoverSpecMark label={label}/>{label}</span>) : <><DiscoverFileCount count={artifact.currentRevision?.fileCount} />{artifact.provenance?.originalFormat ? <Badge variant="outline" title="Source format" className="max-w-full gap-1.5 truncate"><DiscoverFormatMark format={artifact.provenance.originalFormat} />{artifact.provenance.originalFormat}</Badge> : null}</>}
        </div>
      </div>
    </Link>
    {reportedMetrics.length || onFork || onSend || onAdd ? <div className="mx-4 flex flex-wrap items-center gap-x-2.5 gap-y-1 border-t border-[color-mix(in_srgb,var(--aurora-border-default)_45%,transparent)] pb-[13px] pt-2">
      {reportedMetrics.map(({ key, label, icon: MetricIcon, color: metricColor }) => <span key={key} title={label} aria-label={`${artifact.metrics![key]} ${label.toLowerCase()}`} className="inline-flex items-center gap-1 text-[10.5px] font-[650] tabular-nums text-aurora-text-muted"><MetricIcon aria-hidden="true" className="size-[11px]" style={{ color: metricColor }} />{metricLabels?.[key] ?? new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 }).format(artifact.metrics![key]!)}</span>)}
      <span className="min-w-1 flex-1" />
      <span data-discover-actions className="ml-auto inline-flex items-center gap-1">
        {onFork ? <button type="button" aria-label="Fork" title="Fork into your Library" disabled={actionPending} onClick={() => void onFork()} className="grid size-[26px] place-items-center rounded-lg border border-aurora-border-strong/55 bg-aurora-control-surface text-aurora-text-muted transition-colors hover:border-[color-mix(in_srgb,var(--aurora-accent-pink-deep)_55%,var(--aurora-border-strong))] hover:bg-[color-mix(in_srgb,var(--aurora-accent-pink)_9%,var(--aurora-control-surface))] hover:text-aurora-accent-pink disabled:opacity-50"><GitFork aria-hidden className="size-[13px]" strokeWidth={1.7}/></button> : null}
        {onSend ? <button type="button" aria-label="Send to Labby" title="Send to your Labby instance" disabled={actionPending} onClick={() => void onSend()} className="grid size-[26px] place-items-center rounded-lg border border-aurora-border-strong/55 bg-aurora-control-surface text-aurora-text-muted transition-colors hover:border-[color-mix(in_srgb,var(--aurora-accent-primary)_50%,var(--aurora-border-strong))] hover:bg-[color-mix(in_srgb,var(--aurora-accent-primary)_9%,var(--aurora-control-surface))] hover:text-aurora-accent-strong disabled:opacity-50"><Send aria-hidden className="size-[13px]" strokeWidth={1.7}/></button> : null}
        {onAdd ? <button type="button" aria-label={inLibrary ? 'Already in your Library' : 'Add to Library'} title={inLibrary ? 'Already in your Library' : 'Add to Library'} disabled={actionPending || inLibrary} onClick={() => void onAdd()} className={`grid size-[26px] place-items-center rounded-lg border transition-colors disabled:opacity-70 ${inLibrary ? 'border-aurora-success/40 bg-aurora-success/10 text-aurora-success' : 'border-[color-mix(in_srgb,var(--aurora-accent-primary)_50%,var(--aurora-border-strong))] bg-[color-mix(in_srgb,var(--aurora-accent-primary)_12%,var(--aurora-panel-strong))] text-[#bfe7fb] hover:shadow-[0_0_0_1px_color-mix(in_srgb,var(--aurora-accent-primary)_40%,transparent)]'}`}>{actionPending ? <Loader2 aria-hidden className="size-[13px] animate-spin"/> : inLibrary ? <Check aria-hidden className="size-[13px]" strokeWidth={2.2}/> : <Plus aria-hidden className="size-[13px]" strokeWidth={1.9}/>}</button> : null}
      </span>
    </div> : null}
  </article>
}

function DiscoverSpecMark({ label }: { label: string }) {
  const value = label.toLowerCase()
  const Icon = /file|\.md|\.json|\.sh|manifest/.test(value) ? FileText
    : /tool|surface/.test(value) ? Wrench
    : /oauth|blocking|guard|exit/.test(value) ? ShieldCheck
    : /command|slash|arguments|no args/.test(value) ? Terminal
    : /artifact|skill/.test(value) ? Package
    : /token|variable/.test(value) ? Braces
    : /line/.test(value) ? List
    : Clock3
  return <Icon aria-hidden className="size-[11px] shrink-0" strokeWidth={1.7}/>
}
