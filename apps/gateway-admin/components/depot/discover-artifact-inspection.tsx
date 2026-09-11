'use client'

import { useEffect, useRef } from 'react'
import { Check, Copy, Download, GitFork, Link2, Loader2, PackagePlus, Send, Star } from 'lucide-react'
import { AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import { Badge } from '@/components/ui/badge'
import { DiscoverFileCount } from './discover-file-count'
import { DiscoverSourceBadge } from './discover-source-badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
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

export function DiscoverArtifactInspection({ artifact, loading, open, copied, focusKey, importing, inLibrary = false, onImport, onSend, onFork, onOpenChange, onCopy, onExport }: {
  artifact: FederatedArtifact | null
  loading: boolean
  open: boolean
  copied?: string
  focusKey?: string
  importing: boolean
  inLibrary?: boolean
  onImport: (artifact: FederatedArtifact) => Promise<void>
  onSend?: (artifact: FederatedArtifact) => void
  onFork?: (artifact: FederatedArtifact) => void
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
    }} className="max-h-[86vh] w-[96vw] max-w-[720px] gap-0 rounded-aurora-2 border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))] bg-gradient-to-b from-aurora-panel-strong-top to-aurora-panel-strong p-0 text-aurora-text-primary shadow-[var(--aurora-shadow-strong)] sm:max-w-[720px] [&>[data-slot=dialog-close]]:top-[15px] [&>[data-slot=dialog-close]]:grid [&>[data-slot=dialog-close]]:size-7 [&>[data-slot=dialog-close]]:place-items-center [&>[data-slot=dialog-close]]:rounded-lg [&>[data-slot=dialog-close]>svg]:size-3.5">
      <DialogHeader className="shrink-0 border-b border-[color-mix(in_srgb,var(--aurora-border-default)_60%,var(--aurora-page-bg))] px-4 py-[15px] pr-14 text-left">
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
      </DialogHeader>
      {loading ? <div role="status" className="flex min-h-56 items-center justify-center text-sm text-aurora-text-muted"><Loader2 aria-hidden="true" className="mr-[var(--space-2)] size-4 animate-spin" />Loading artifact…</div>
        : artifact ? <>
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
              {([['Source format', artifact.provenance?.originalFormat], ['Source format version', artifact.provenance?.originalVersion], ['Declared license', artifact.license?.declared], ['License review', artifact.license?.reviewState], ['Distribution', artifact.publication?.distribution], ['Redistribution', artifact.license?.redistribution], ['Revisions', artifact.revisionCount?.toString()]] as const).map(([label, value]) =>
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
            <Button variant="outline" size="sm" title="Export metadata" onClick={() => onExport(artifact)}><Download aria-hidden="true"/><span className="sr-only">Export metadata</span></Button>
            <Button variant="outline" size="sm" title="Copy link" onClick={() => onCopy('Artifact link', window.location.href)}><Link2 aria-hidden="true"/><span className="sr-only">Copy link</span></Button>
            {onFork ? <Button variant="outline" size="sm" title="Fork into your Library" className="border-aurora-accent-pink-deep/40 bg-aurora-accent-pink/10 text-aurora-accent-pink" onClick={() => onFork(artifact)}><GitFork aria-hidden="true"/><span className="sr-only">Fork</span></Button> : null}
            {onSend ? <Button variant="outline" size="sm" title="Send to your Labby instance" className="border-aurora-accent-primary/40 bg-aurora-accent-primary/10 text-aurora-accent-strong" onClick={() => onSend(artifact)}><Send aria-hidden="true"/><span className="sr-only">Send to Labby</span></Button> : null}
            <span aria-hidden="true" className="mx-0.5 h-5 w-px bg-aurora-border-default"/>
            <Button size="sm" title={inLibrary ? 'In Library' : importing ? 'Adding…' : 'Add to Library'} disabled={inLibrary || importing || !(artifact.currentRevisionId || artifact.currentRevision?.id)} onClick={() => void onImport(artifact)}>
              {inLibrary ? <Check aria-hidden="true"/> : importing ? <Loader2 aria-hidden="true" className="animate-spin"/> : <PackagePlus aria-hidden="true"/>}<span className="sr-only">{inLibrary ? 'In Library' : importing ? 'Adding…' : 'Add to Library'}</span>
            </Button>
          </DialogFooter>
        </> : <p role="status" className="p-[var(--space-7)] text-sm text-aurora-text-muted">Artifact details are unavailable. Close this dialog and retry from the catalog.</p>}
    </DialogContent>
  </Dialog>
}
