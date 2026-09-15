'use client'

import { useEffect, useRef, useState } from 'react'
import Link from 'next/link'
import { BookOpen, Braces, Check, ChevronRight, Clock3, Copy, Download, FileText, GitFork, Link2, List, Loader2, MessageSquare, Package, PackagePlus, Plus, Send, ShieldCheck, Star, Terminal, Wrench, X } from 'lucide-react'
import { AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import { Badge } from '@/components/ui/badge'
import { DiscoverFileCount } from './discover-file-count'
import { DiscoverSourceBadge } from './discover-source-badge'
import { Button } from '@/components/ui/button'
import { Popover, PopoverClose, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import type { FederatedArtifact } from '@/lib/api/depot-client'
import { artifactKind, artifactTitle } from './discover-model'
import { discoverKindPresentation, DiscoverPublisherVerifiedIcon } from './discover-kind-presentation'
import { DiscoverReadme } from './discover-readme'
import { DiscoverUpstream } from './discover-upstream'

const readmeUnavailable = {
  absent: 'This revision has no README.md or SKILL.md file.',
  not_distributable: 'This source permits metadata access but does not permit a document preview.',
  too_large: 'The source document exceeds the 64 KiB preview limit.',
  storage_unavailable: 'The source document could not be verified or retrieved. Retry the Artifact details later.',
  invalid_text: 'The source document is not valid UTF-8 text.',
} as const

export function DiscoverArtifactInspection({ artifact, previewMode = false, specLabels, metricLabels, inLibrary = false, loading, open, copied, focusKey, importing, onImport, onFork, onSend, installFormats, onInstallFormat, onOpenChange, onCopy, onExport }: {
  artifact: FederatedArtifact | null
  previewMode?: boolean
  specLabels?: readonly string[]
  metricLabels?: Partial<Record<'stars' | 'installs' | 'forks', string>>
  inLibrary?: boolean
  loading: boolean
  open: boolean
  copied?: string
  focusKey?: string
  importing: boolean
  onImport: (artifact: FederatedArtifact) => Promise<void>
  onFork?: (artifact: FederatedArtifact) => void | Promise<void>
  onSend?: (artifact: FederatedArtifact) => void | Promise<void>
  installFormats?: readonly string[]
  onInstallFormat?: (artifact: FederatedArtifact, format: string) => void | Promise<void>
  onOpenChange: (open: boolean) => void
  onCopy: (label: string, value?: string) => void
  onExport: (artifact: FederatedArtifact) => void
}) {
  const returnFocusKey = useRef(focusKey)
  useEffect(() => { if (focusKey) returnFocusKey.current = focusKey }, [focusKey])
  const title = artifact ? artifactTitle(artifact) : 'Artifact details'
  const kind = artifact ? artifactKind(artifact) : 'artifact'
  const { family, tone, icon: KindIcon, iconStyle } = discoverKindPresentation(kind)
  const values = artifact ? [
    ['Artifact ID', artifact.artifactId],
    ['Revision ID', artifact.currentRevisionId ?? artifact.currentRevision?.id],
    ['Content digest', artifact.contentDigest ?? artifact.currentRevision?.contentDigest],
  ] as const : []
  // Provenance rows appear only when the source supplied them; the policy rows always render so an absent value is visible.
  const detailRows: ReadonlyArray<readonly [string, string | null | undefined]> = artifact ? [
    ...([['Source format', artifact.provenance?.originalFormat], ['Source format version', artifact.provenance?.originalVersion]] as const).filter(([, value]) => value),
    ['Declared license', artifact.license?.declared],
    ['License review', artifact.license?.reviewState],
    ['Distribution', artifact.publication?.distribution],
    ['Redistribution', artifact.license?.redistribution],
    ['Revisions', artifact.revisionCount?.toString()],
  ] : []
  const metrics = [
    { key: 'stars', label: 'Stars', icon: Star, color: 'var(--aurora-warn)' },
    { key: 'installs', label: 'Installs', icon: Download, color: 'var(--aurora-accent-strong)' },
    { key: 'forks', label: 'Forks', icon: GitFork, color: 'var(--aurora-accent-pink)' },
  ] as const

  return <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent onCloseAutoFocus={event => {
      event.preventDefault()
      const card = Array.from(document.querySelectorAll<HTMLAnchorElement>('a[data-artifact-key]')).find(element => element.dataset.artifactKey === returnFocusKey.current)
      const target = card ?? document.querySelector<HTMLInputElement>('input[name="artifact-search"]')
      target?.focus()
    }} showCloseButton={!previewMode} overlayClassName={previewMode ? '!bg-[rgba(4,12,18,0.62)]' : undefined} style={previewMode ? { width: 'min(720px, calc(100vw - 50px))', maxWidth: 'min(720px, calc(100vw - 50px))', boxSizing: 'content-box' } : undefined} className="max-h-[86vh] w-[96vw] max-w-[720px] gap-0 rounded-aurora-2 border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))] bg-gradient-to-b from-aurora-panel-strong-top to-aurora-panel-strong p-0 text-aurora-text-primary shadow-[var(--aurora-shadow-strong),inset_0_1px_0_rgba(255,255,255,0.05)] sm:max-w-[720px] [&>[data-slot=dialog-close]]:top-[15px] [&>[data-slot=dialog-close]]:grid [&>[data-slot=dialog-close]]:size-7 [&>[data-slot=dialog-close]]:place-items-center [&>[data-slot=dialog-close]]:rounded-lg [&>[data-slot=dialog-close]>svg]:size-3.5">
      {previewMode && artifact ? <PreviewArtifactHeader artifact={artifact}/> : <DialogHeader className="shrink-0 border-b border-[color-mix(in_srgb,var(--aurora-border-default)_60%,var(--aurora-page-bg))] px-4 py-[15px] pr-14 text-left">
        <div className="flex min-w-0 items-start gap-[11px]">
          <span aria-hidden="true" style={iconStyle} className="grid size-[34px] shrink-0 place-items-center rounded-[10px] border"><KindIcon className="size-[18px]" /></span>
          <div className="min-w-0">
            <div className="flex min-w-0 items-center gap-[7px]"><DialogTitle className="truncate font-display text-[18px] font-extrabold tracking-[-.01em] text-aurora-text-primary" title={title}>{title}</DialogTitle>{artifact?.publisherVerified === true ? <DiscoverPublisherVerifiedIcon aria-label="Verified publisher" className="size-3.5 shrink-0 text-aurora-success"/> : null}</div>
            <DialogDescription className="sr-only">
              {artifact ? `${artifact.namespace ?? artifact.descriptor?.namespace ?? 'Publisher not supplied'} · ${artifact.providerId}` : 'Inspect the selected artifact and its source revision.'}
            </DialogDescription>
            {artifact?.descriptor?.tags?.length ? <ul aria-label="Artifact tags" className="mt-1 flex flex-wrap gap-[5px]">
              {artifact.descriptor.tags.map(tag => <li key={tag} className="min-w-0 max-w-full"><span className="inline-flex min-h-5 max-w-full items-center rounded-[5px] border border-[color-mix(in_srgb,var(--aurora-accent-primary)_22%,transparent)] bg-[color-mix(in_srgb,var(--aurora-accent-primary)_8%,transparent)] px-[9px] text-[10.5px] font-[650] break-all text-aurora-accent-strong">{tag}</span></li>)}
            </ul> : null}
          </div>
        </div>
      </DialogHeader>}
      {loading ? <div role="status" className="flex min-h-56 items-center justify-center text-sm text-aurora-text-muted"><Loader2 aria-hidden="true" className="mr-[var(--space-2)] size-4 animate-spin" />Loading artifact…</div>
        : artifact ? previewMode ? <PreviewArtifactDetail artifact={artifact} specLabels={specLabels} metricLabels={metricLabels} inLibrary={inLibrary} copied={copied} importing={importing} onImport={onImport} onFork={onFork} onSend={onSend} installFormats={installFormats} onInstallFormat={onInstallFormat} onCopy={onCopy}/> : <>
          <div className="aurora-scrollbar min-h-0 flex-1 space-y-3.5 overflow-y-auto px-4 pt-[15px] pb-5">
            <p className="text-[13px] leading-[1.6] text-pretty">{artifact.description ?? artifact.descriptor?.description ?? 'No description supplied by this source.'}</p>
            <div style={{ backgroundColor: `color-mix(in srgb, ${tone} 7%, transparent)`, borderColor: `color-mix(in srgb, ${tone} 24%, transparent)` }} className="flex flex-wrap gap-2 rounded-[10px] border px-[11px] py-[9px]">
              <Badge variant="outline" style={{ color: iconStyle.color }}><KindIcon aria-hidden="true" className="size-3" />{kind}</Badge>
              {family ? <span className={`${AURORA_MUTED_LABEL} self-center`}>{family}</span> : null}
              <DiscoverFileCount count={artifact.currentRevision?.fileCount} />
              {artifact.publication?.state ? <Badge variant="outline">{artifact.publication.state}</Badge> : null}
              {artifact.publication?.visibility ? <Badge variant="outline">{artifact.publication.visibility}</Badge> : null}
              {artifact.currentRevision?.authoredAt ? <span className="self-center text-xs text-aurora-text-muted">Revision authored {artifact.currentRevision.authoredAt}</span> : null}
            </div>
            {artifact.readme?.state === 'available' ? <DiscoverReadme path={artifact.readme.path} content={artifact.readme.content} /> : artifact.readme?.state === 'unavailable' ? <section aria-label="Document preview" className="rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-medium p-[var(--space-4)]">
              <h3 className={AURORA_MUTED_LABEL}>Document preview</h3>
              <p className="mt-[var(--space-2)] text-sm text-aurora-text-muted">{readmeUnavailable[artifact.readme.reason]}</p>
            </section> : null}
            <DiscoverUpstream artifact={artifact} />
            <details key={`${artifact.providerId}:${artifact.artifactId}`} className="rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-medium">
              <summary className={`${AURORA_MUTED_LABEL} cursor-pointer rounded-aurora-2 p-[var(--space-4)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary`}>Source and revision</summary>
              <div className="space-y-[var(--space-4)] p-[var(--space-4)]">
                {artifact.sourceOrigin ? <DiscoverSourceBadge artifact={artifact} /> : null}
                {values.map(([label, value]) => value ? <div key={label} className="flex min-w-0 items-center gap-[var(--space-2)]">
                  <div className="min-w-0 flex-1"><p className={AURORA_MUTED_LABEL}>{label}</p><code className="mt-[var(--space-1)] block break-all text-xs">{value}</code></div>
                  <Button variant="ghost" size="sm" aria-label={`Copy ${label}`} title={`Copy ${label}`} onClick={() => onCopy(label, value)}>
                    {copied === label ? <Check aria-hidden="true" className="size-4 text-aurora-success" /> : <Copy aria-hidden="true" className="size-4" />}Copy
                  </Button>
                </div> : null)}
            <dl className="grid grid-cols-2 gap-[var(--space-4)] text-xs">
              {detailRows.map(([label, value]) =>
                <div key={label} className="min-w-0"><dt className={AURORA_MUTED_LABEL}>{label}</dt><dd className="mt-[var(--space-2)] break-words text-aurora-text-muted">{value || 'Not supplied'}</dd></div>
              )}
            </dl>
              </div>
            </details>
          </div>
          <DialogFooter className="shrink-0 flex-row flex-wrap items-center gap-1.5 border-t border-aurora-border-default bg-aurora-control-surface px-4 pt-[9px] pb-[11px] [&>button]:size-8 [&>button]:rounded-[9px] [&>button]:p-0 [&>button>svg]:size-3.5">
            <div className="mr-auto flex flex-wrap items-center gap-3">
              {metrics.map(({ key, label, icon: MetricIcon, color }) => {
                const value = artifact.metrics?.[key]
                if (!Number.isSafeInteger(value) || value! < 0) return null
                return <span key={key} title={label} aria-label={`${value} ${label.toLowerCase()}`} className="inline-flex items-center gap-[5px] text-[11.5px] font-bold tabular-nums"><MetricIcon aria-hidden="true" className="size-3" style={{ color }}/>{new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 }).format(value!)}</span>
              })}
            </div>
            {onFork?<Button variant="outline" size="sm" title="Fork into your Library" aria-label="Fork into your Library" onClick={()=>void onFork(artifact)} className="border-aurora-accent-pink/40 bg-aurora-accent-pink/10 text-aurora-accent-pink hover:text-aurora-accent-pink"><GitFork aria-hidden="true"/></Button>:null}
            {onSend?<Button variant="outline" size="sm" title="Send to your Labby instance" aria-label="Send to your Labby instance" onClick={()=>void onSend(artifact)} className="border-aurora-accent-primary/40 bg-aurora-accent-primary/10 text-aurora-accent-strong"><Send aria-hidden="true"/></Button>:null}
            {installFormats?.length&&onInstallFormat?<Popover><PopoverTrigger asChild><Button variant="outline" size="sm" title="Install formats — compiled by APM" aria-label="Install formats" className="border-aurora-success/40 bg-aurora-success/10 text-aurora-success"><Download aria-hidden="true"/></Button></PopoverTrigger><PopoverContent align="end" side="top" className="w-[236px] rounded-[12px] border-aurora-border-strong bg-aurora-panel-strong p-[5px]"><p className="px-2 pb-[3px] pt-1 text-[9px] font-bold uppercase tracking-[0.13em] text-aurora-text-muted">Compile via APM</p>{installFormats.map(format=><Button key={format} variant="ghost" className="h-8 w-full justify-start rounded-lg px-2 text-[11.5px] font-semibold" onClick={()=>void onInstallFormat(artifact,format)}>{format}</Button>)}</PopoverContent></Popover>:null}
            <Button variant="outline" size="sm" title="Export metadata" onClick={() => onExport(artifact)}><Download aria-hidden="true"/><span className="sr-only">Export metadata</span></Button>
            <Button variant="outline" size="sm" title="Copy link" onClick={() => onCopy('Artifact link', window.location.href)}><Link2 aria-hidden="true"/><span className="sr-only">Copy link</span></Button>
            <span aria-hidden="true" className="mx-0.5 h-5 w-px bg-aurora-border-default"/>
            <Button size="sm" title={importing ? 'Adding…' : 'Add to Library'} disabled={importing || !(artifact.currentRevisionId || artifact.currentRevision?.id)} onClick={() => void onImport(artifact)}>
              {importing ? <Loader2 aria-hidden="true" className="animate-spin"/> : <PackagePlus aria-hidden="true"/>}<span className="sr-only">{importing ? 'Adding…' : 'Add to Library'}</span>
            </Button>
          </DialogFooter>
        </> : <p role="status" className="p-[var(--space-7)] text-sm text-aurora-text-muted">Artifact details are unavailable. Close this dialog and retry from the catalog.</p>}
    </DialogContent>
  </Dialog>
}

function PreviewArtifactHeader({ artifact }: { artifact: FederatedArtifact }) {
  const title = artifactTitle(artifact)
  const kind = artifactKind(artifact)
  const { icon: KindIcon, iconStyle } = discoverKindPresentation(kind)
  const tags = artifact.descriptor?.tags ?? []
  return <div data-preview-artifact-header className="flex shrink-0 items-start gap-[11px] border-b border-[color-mix(in_srgb,var(--aurora-border-default)_60%,var(--aurora-page-bg))] px-4 py-[15px]">
    <span aria-hidden="true" style={iconStyle} className="grid size-9 shrink-0 place-items-center rounded-[10px] border"><KindIcon className="size-[18px]" /></span>
    <div className="flex min-w-0 flex-1 flex-col gap-[3px]">
      <div className="flex h-6 min-w-0 items-center gap-[7px]"><DialogTitle className="truncate font-display text-[18px] font-extrabold leading-6 tracking-[-0.01em] text-aurora-text-primary" title={title}>{title}</DialogTitle>{artifact.publisherVerified === true ? <DiscoverPublisherVerifiedIcon aria-label="Verified publisher" className="size-3.5 shrink-0 text-aurora-success"/> : null}</div>
      {tags.length ? <div aria-label="Artifact tags" className="mt-px flex h-[22px] flex-wrap items-center gap-[5px]">{tags.map(tag=><span key={tag} className="inline-flex h-5 max-w-full items-center rounded-[5px] border border-[color-mix(in_srgb,var(--aurora-accent-primary)_22%,transparent)] bg-[color-mix(in_srgb,var(--aurora-accent-primary)_8%,transparent)] px-[9px] text-[10.5px] font-[650] text-aurora-accent-strong">#{tag}</span>)}</div> : null}
    </div>
    <div className="flex shrink-0 items-start gap-2"><DialogClose aria-label="Close" title="Close — esc" className="grid size-7 shrink-0 place-items-center rounded-lg text-aurora-text-muted transition-colors hover:bg-aurora-hover-bg hover:text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary"><X aria-hidden className="size-3.5" strokeWidth={1.9}/></DialogClose></div>
  </div>
}

type PreviewArtifactDetailProps = {
  artifact: FederatedArtifact
  specLabels?: readonly string[]
  metricLabels?: Partial<Record<'stars' | 'installs' | 'forks', string>>
  inLibrary: boolean
  copied?: string
  importing: boolean
  onImport: (artifact: FederatedArtifact) => Promise<void>
  onFork?: (artifact: FederatedArtifact) => void | Promise<void>
  onSend?: (artifact: FederatedArtifact) => void | Promise<void>
  installFormats?: readonly string[]
  onInstallFormat?: (artifact: FederatedArtifact, format: string) => void | Promise<void>
  onCopy: (label: string, value?: string) => void
}

function PreviewArtifactDetail({ artifact, specLabels = [], metricLabels, inLibrary, copied, importing, onImport, onFork, onSend, installFormats, onInstallFormat, onCopy }: PreviewArtifactDetailProps) {
  const [contentsOpen, setContentsOpen] = useState(false)
  useEffect(() => setContentsOpen(false), [artifact.providerId, artifact.artifactId])
  const kind = artifactKind(artifact)
  const { family, tone, color, icon: KindIcon } = discoverKindPresentation(kind)
  const description = artifact.description ?? artifact.descriptor?.description ?? 'No description supplied by this source.'
  const publisher = artifact.namespace ?? artifact.descriptor?.namespace ?? 'community'
  const artifactName = artifact.name ?? artifact.title ?? artifact.artifactId
  const installRef = publisher.includes('/') ? publisher : `${publisher}/${artifactName}`
  const installCommand = `depot add ${installRef}`
  const sourceLabel = artifact.provenance?.originalFormat ?? artifact.sourceOrigin ?? artifact.providerId
  const contents = previewArtifactContents(artifact, specLabels)
  const readmePath = artifact.readme?.state === 'available' ? artifact.readme.path : kind === 'skill' ? 'SKILL.md' : 'README.md'
  const metrics = [
    { key: 'stars', label: 'Stars', icon: Star, color: 'var(--aurora-warn)' },
    { key: 'installs', label: 'Installs', icon: Download, color: 'var(--aurora-accent-strong)' },
    { key: 'forks', label: 'Forks', icon: GitFork, color: 'var(--aurora-accent-pink)' },
  ] as const
  return <>
    <div className="aurora-scrollbar min-h-0 flex-1 flex-col gap-[14px] overflow-y-auto px-4 pb-5 pt-[15px] flex">
      <p className="m-0 text-pretty text-[13px] leading-[1.6] text-aurora-text-primary">{description}</p>
      <div style={{ backgroundColor: `color-mix(in srgb, ${tone} 7%, transparent)`, borderColor: `color-mix(in srgb, ${tone} 24%, transparent)` }} className="flex shrink-0 flex-col overflow-hidden rounded-[10px] border">
        <button type="button" aria-expanded={contentsOpen} disabled={!contents.length} onClick={()=>contents.length&&setContentsOpen(open=>!open)} className="flex flex-wrap items-center gap-2 border-0 bg-transparent px-[11px] py-[9px] text-left font-sans leading-[13px] disabled:cursor-default">
          <KindIcon aria-hidden className="size-[13px] shrink-0" style={{ color }}/>
          <span className="shrink-0 text-[9.5px] font-bold uppercase leading-[13px] tracking-[0.11em]" style={{ color }}>{kind}</span>
          {family ? <span className="shrink-0 text-[9px] font-[650] uppercase leading-[13px] tracking-[0.1em] text-[#95b4c7]">{family}</span> : null}
          <span aria-hidden className="h-3 w-px shrink-0" style={{ background: `color-mix(in srgb, ${tone} 30%, transparent)` }}/>
          {specLabels.map(label=><span key={label} className="inline-flex min-w-0 items-center gap-[5px]"><PreviewSpecMark label={label} color={color}/><span className="whitespace-nowrap text-[10.5px] font-semibold leading-[13px] text-aurora-text-muted">{label}</span></span>)}
          <span className="min-w-1 flex-1"/>
          {contents.length?<ChevronRight aria-hidden className={`size-3 shrink-0 transition-transform ${contentsOpen?'rotate-90':''}`} strokeWidth={2} style={{color}}/>:null}
        </button>
        {contentsOpen?<div className="flex flex-col border-t" style={{borderColor:`color-mix(in srgb, ${tone} 22%, transparent)`}}>{contents.map(([name,role])=><div key={name} className="flex min-w-0 items-center gap-[9px] border-b border-[color-mix(in_srgb,var(--aurora-border-default)_28%,transparent)] px-3 py-[7px] last:border-b-0"><PreviewContentMark role={role}/><span className="min-w-0 flex-1 truncate text-[11.5px] font-semibold leading-[14px] text-aurora-text-primary">{name}</span><span className="shrink-0 text-[9px] font-bold uppercase tracking-[0.08em] text-aurora-text-muted">{role}</span></div>)}</div>:null}
      </div>
      <section aria-label="Readme" className="flex shrink-0 flex-col overflow-hidden rounded-[12px] border border-[color-mix(in_srgb,var(--aurora-border-default)_50%,var(--aurora-page-bg))] bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-[inset_0_1px_0_rgba(255,255,255,0.04)]">
        <div className="flex items-center gap-2 border-b border-[color-mix(in_srgb,var(--aurora-border-default)_50%,var(--aurora-page-bg))] bg-[var(--gw0-0_38)] px-[13px] py-[9px]">
          <BookOpen aria-hidden className="size-[13px] shrink-0 text-aurora-accent-strong" strokeWidth={1.7}/><span className="shrink-0 text-[9.5px] font-bold uppercase leading-[13px] tracking-[0.13em] text-aurora-text-muted">Readme</span><span className="min-w-1 flex-1"/><code className="font-mono text-[10px] leading-[13px] text-[#99b8cb]">{readmePath}</code>
        </div>
        <div className="flex flex-col gap-[11px] px-[14px] pb-[15px] pt-[13px]">
          <div className="flex min-w-0 flex-col gap-1.5 pr-[14px]"><p className="m-0 text-pretty text-[12.5px] leading-[1.65] text-aurora-text-muted">{description}</p></div>
          <div className="flex min-w-0 flex-col gap-1.5 pr-[14px]">
            <PreviewReadmeHeading>Install</PreviewReadmeHeading>
            <div className="flex min-w-0 items-center gap-[9px] rounded-[9px] border border-[color-mix(in_srgb,var(--aurora-border-strong)_60%,var(--aurora-page-bg))] bg-[var(--gw0-0_48)] px-[10px] py-2">
              <span className="shrink-0 font-mono text-[11.5px] leading-[14px] text-aurora-accent-strong">$</span><code className="min-w-0 overflow-x-auto whitespace-nowrap font-mono text-[11.5px] leading-[14px] text-aurora-text-primary">{installCommand}</code><span className="min-w-1 flex-1"/><button type="button" aria-label="Copy install command" title="Copy" onClick={()=>onCopy('Install command',installCommand)} className="grid size-6 shrink-0 place-items-center rounded-[7px] text-aurora-text-muted transition-colors hover:bg-aurora-hover-bg hover:text-aurora-accent-strong focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">{copied==='Install command'?<Check aria-hidden className="size-3 text-aurora-success"/>:<Copy aria-hidden className="size-3" strokeWidth={1.8}/>}</button>
            </div>
          </div>
          <div className="flex min-w-0 flex-col gap-1.5 pr-[14px]"><PreviewReadmeHeading>Upstream</PreviewReadmeHeading><p className="m-0 text-pretty text-[12.5px] leading-[1.65] text-aurora-text-muted">Tracked from {sourceLabel}. Forks stay linked {'\u2014'} Depot surfaces every upstream change as a reviewable diff.</p></div>
        </div>
      </section>
    </div>
    <div className="flex shrink-0 flex-wrap items-center gap-[10px] border-t border-[color-mix(in_srgb,var(--aurora-border-default)_60%,var(--aurora-page-bg))] bg-[var(--gw0-0_38)] px-4 pb-[11px] pt-[9px]">
      <div className="mt-[3px] flex flex-wrap items-center gap-3">{metrics.map(({key,label,icon:MetricIcon,color:metricColor})=>{const value=artifact.metrics?.[key];if(!Number.isSafeInteger(value)||value!<0)return null;return <span key={key} title={label} aria-label={`${value} ${label.toLowerCase()}`} className="inline-flex min-w-0 items-center gap-[5px]"><MetricIcon aria-hidden className="size-3 shrink-0" style={{color:metricColor}}/><span className="text-[11.5px] font-bold leading-[14px] tabular-nums text-aurora-text-primary">{metricLabels?.[key]??new Intl.NumberFormat('en',{notation:'compact',maximumFractionDigits:1}).format(value!)}</span></span>})}</div>
      <span className="min-w-2 flex-1"/>
      <div className="flex shrink-0 flex-wrap items-center justify-end gap-1.5">
        {onFork?<Button data-visible-label variant="outline" title="Fork into your Library" aria-label="Fork into your Library" onClick={()=>void onFork(artifact)} className="size-8 rounded-[9px] border-[color-mix(in_srgb,var(--aurora-accent-pink-deep)_42%,transparent)] bg-[color-mix(in_srgb,var(--aurora-accent-pink)_9%,transparent)] p-0 text-aurora-accent-pink"><GitFork aria-hidden className="size-3.5" strokeWidth={1.7}/></Button>:null}
        {onSend?<Button data-visible-label variant="outline" title="Send to your Labby instance" aria-label="Send to your Labby instance" onClick={()=>void onSend(artifact)} className="size-8 rounded-[9px] border-[color-mix(in_srgb,var(--aurora-accent-primary)_42%,transparent)] bg-[color-mix(in_srgb,var(--aurora-accent-primary)_9%,transparent)] p-0 text-aurora-accent-strong"><Send aria-hidden className="size-3.5" strokeWidth={1.7}/></Button>:null}
        {installFormats?.length&&onInstallFormat?<Popover><PopoverTrigger asChild><Button data-visible-label variant="outline" title="Install formats — compiled by APM" aria-label="Install formats" className="size-8 rounded-[9px] border-aurora-success/40 bg-aurora-success/10 p-0 text-aurora-success"><Download aria-hidden className="size-3.5" strokeWidth={1.7}/></Button></PopoverTrigger><PopoverContent align="end" side="top" sideOffset={5} className="box-content w-[236px] rounded-[12px] border-[color-mix(in_srgb,var(--aurora-border-strong)_75%,var(--aurora-page-bg))] bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] p-[5px] shadow-[var(--aurora-shadow-strong),inset_0_1px_0_rgba(255,255,255,0.05)]"><span className="block px-2 pb-[3px] pt-1 text-[9px] font-bold uppercase tracking-[0.13em] text-[#97b6c9]">Compile Via APM</span>{installFormats.map(format=><PopoverClose asChild key={format}><Button data-visible-label variant="ghost" className="flex w-full items-center justify-start gap-2 rounded-lg px-2 py-[7px] text-[11.5px] font-[650]" onClick={()=>void onInstallFormat(artifact,format)}><PreviewTargetMark label={format}/><span className="truncate">{format}</span></Button></PopoverClose>)}</PopoverContent></Popover>:null}
        <span aria-hidden className="mx-0.5 h-5 w-px shrink-0 bg-[color-mix(in_srgb,var(--aurora-border-default)_70%,transparent)]"/>
        {inLibrary?<Button data-visible-label asChild title="Already in your Library" className="size-8 rounded-[9px] border border-aurora-success/40 bg-aurora-success/10 p-0 text-aurora-success"><Link href="/library" aria-label="Already in your Library"><Check aria-hidden className="size-3.5" strokeWidth={2.2}/></Link></Button>:<Button data-visible-label title={importing?'Adding…':'Add to Library'} aria-label={importing?'Adding…':'Add to Library'} disabled={importing||!(artifact.currentRevisionId||artifact.currentRevision?.id)} onClick={()=>void onImport(artifact)} className="size-8 rounded-[9px] border border-[color-mix(in_srgb,var(--aurora-accent-primary)_50%,var(--aurora-border-strong))] bg-[color-mix(in_srgb,var(--aurora-accent-primary)_12%,var(--aurora-panel-strong))] p-0 text-[#bfe7fb]">{importing?<Loader2 aria-hidden className="size-3.5 animate-spin"/>:<Plus aria-hidden className="size-3.5" strokeWidth={1.9}/>}</Button>}
      </div>
    </div>
  </>
}

function previewArtifactContents(artifact: FederatedArtifact, specLabels: readonly string[]): ReadonlyArray<readonly [string, string]> {
  const name=artifact.name??artifact.title??artifact.artifactId
  const slug=name.replace(/[^a-z0-9-]/gi,'')
  const n=name.length
  const seed=(x:number)=>2+((n*7+x)%7)
  const toolLabel=specLabels.find(label=>/^\d+ tools$/.test(label))
  const toolCount=toolLabel?Number.parseInt(toolLabel,10):4
  const bundleFiles:Record<string,ReadonlyArray<readonly [string,string]>>={
    'review-suite':[['plugin.json','manifest'],['rust-reviewer.md','agent'],['repo-triage/SKILL.md','skill'],['pre-commit-guard.sh','hook'],['README.md','doc']],
    'homelab-pack':[['plugin.json','manifest'],['repo-triage/SKILL.md','skill'],['secrets-sweeper/SKILL.md','skill'],['doc-crawler/SKILL.md','skill'],['unraid-ops.json','mcp'],['ship.md','command']],
    'project-a-loadout':[['loadout.toml','manifest'],['rust-reviewer.md','agent'],['repo-triage/SKILL.md','skill'],['changelog-writer/SKILL.md','skill'],['ship.md','command']],
    'oncall-loadout':[['loadout.toml','manifest'],['incident-postmortem.md','prompt'],['cost-ceiling.sh','hook'],['scope-audit.md','command']],
  }
  if(bundleFiles[name])return bundleFiles[name]
  const kind=artifactKind(artifact).toLowerCase()
  if(kind==='mcp'){
    const stem=slug.replace(/-(mcp|ops|server)$/,'').replace(/-/g,'_')||'server'
    const verbs=['list','get','search','exec','create','update','delete','watch','stat'].slice(0,Math.min(Math.max(1,toolCount),9)).map(v=>[`${stem}_${v}`,'tool'] as const)
    return [...verbs,...([[`${stem}://status`,'resource'],[`${stem}://config`,'resource']] as const).slice(0,1+(seed(3)%2)),[`summarize_${stem}`,'prompt']]
  }
  if(kind==='acp'){
    const stem=slug.replace(/-(acp|bridge)$/,'').replace(/-/g,'_')||'bridge'
    return [...['open','apply_edit','diff','select','close'].slice(0,Math.min(Math.max(2,toolCount),5)).map(v=>[`${stem}_${v}`,'tool'] as const),[`${stem}://buffer`,'resource'],[`review_${stem}`,'prompt']]
  }
  if(kind==='skill')return ([['SKILL.md','manifest'],['reference.md','doc'],[`scripts/${slug}.sh`,'hook'],['examples/basic.md','doc']] as const).slice(0,seed(1)%2?4:3)
  if(kind==='agent')return [[`${slug}.md`,'agent'],['tools.json','manifest'],['prompts/system.md','prompt']]
  if(kind==='command')return [[`${slug}.md`,'command'],['args.json','manifest']]
  if(kind==='hook')return [[`${slug}.sh`,'hook'],['hook.json','manifest']]
  if(kind==='prompt')return [[`${slug}.md`,'prompt'],['variables.json','manifest']]
  if(kind==='extension'){
    const stem=slug.replace(/-(ext|extension)$/,'').replace(/-/g,'_')||'ext'
    return [...['query','cite','fetch','summarize'].slice(0,Math.min(Math.max(2,toolCount),4)).map(v=>[`${stem}_${v}`,'tool'] as const),[`${stem}://index`,'resource'],[`ask_${stem}`,'prompt']]
  }
  if(kind==='snippet')return [[`${slug}.labby`,'manifest'],['README.md','doc']]
  return []
}

function PreviewContentMark({ role }: { role: string }) {
  const Icon=role==='tool'?Wrench:role==='resource'?Package:role==='prompt'?MessageSquare:role==='hook'?ShieldCheck:role==='command'?Terminal:FileText
  return <Icon aria-hidden className="size-3 shrink-0 text-aurora-text-muted" strokeWidth={1.7}/>
}

function PreviewReadmeHeading({ children }: { children: React.ReactNode }) {
  return <span className="inline-flex items-center gap-[7px] font-display text-[12.5px] font-[760] leading-[17px] tracking-[-0.005em] text-aurora-text-primary"><span aria-hidden className="h-3 w-[3px] rounded-full bg-[color-mix(in_srgb,var(--aurora-accent-primary)_65%,transparent)]"/>{children}</span>
}

function PreviewSpecMark({ label, color }: { label: string; color: string }) {
  const value=label.toLowerCase()
  const Icon=/file|\.md|\.json|\.sh|manifest/.test(value)?FileText:/tool|surface/.test(value)?Wrench:/oauth|blocking|guard|exit/.test(value)?ShieldCheck:/command|slash|arguments|no args/.test(value)?Terminal:/artifact|skill/.test(value)?Package:/token|variable/.test(value)?Braces:/line/.test(value)?List:Clock3
  return <Icon aria-hidden className="size-[11px] shrink-0" strokeWidth={1.7} style={{color}}/>
}

function PreviewTargetMark({ label }: { label: string }) {
  const value=label.toLowerCase()
  const Icon=value.includes('mcp')?Wrench:value.includes('ard')?ShieldCheck:value.includes('loadout')?Package:value.includes('plugin')?Package:FileText
  return <Icon aria-hidden className="size-[13px] shrink-0 text-aurora-accent-strong" strokeWidth={1.7}/>
}
