'use client'

import { useEffect, useRef } from 'react'
import { Check, Copy, Download, Layers3, Link2, Loader2, PackagePlus } from 'lucide-react'
import { AURORA_DISPLAY_2, AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import type { FederatedArtifact } from '@/lib/api/depot-client'
import { artifactKind, artifactTitle } from './discover-model'

export function DiscoverArtifactInspection({ artifact, loading, open, copied, focusKey, importing, onImport, onOpenChange, onCopy, onExport }: {
  artifact: FederatedArtifact | null
  loading: boolean
  open: boolean
  copied?: string
  focusKey?: string
  importing: boolean
  onImport: (artifact: FederatedArtifact) => Promise<void>
  onOpenChange: (open: boolean) => void
  onCopy: (label: string, value?: string) => void
  onExport: (artifact: FederatedArtifact) => void
}) {
  const returnFocusKey = useRef(focusKey)
  useEffect(() => { if (focusKey) returnFocusKey.current = focusKey }, [focusKey])
  const title = artifact ? artifactTitle(artifact) : 'Artifact details'
  const values = artifact ? [
    ['Artifact ID', artifact.artifactId],
    ['Revision ID', artifact.currentRevisionId ?? artifact.currentRevision?.id],
    ['Content digest', artifact.contentDigest ?? artifact.currentRevision?.contentDigest],
  ] as const : []

  return <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent onCloseAutoFocus={event => {
      event.preventDefault()
      const card = Array.from(document.querySelectorAll<HTMLAnchorElement>('a[data-artifact-key]')).find(element => element.dataset.artifactKey === returnFocusKey.current)
      const target = card ?? document.querySelector<HTMLInputElement>('input[name="artifact-search"]')
      target?.focus()
    }} className="gap-0 rounded-aurora-3 border-aurora-border-strong bg-aurora-panel-strong p-0 text-aurora-text-primary shadow-[var(--aurora-shadow-strong)] sm:max-w-3xl">
      <DialogHeader className="shrink-0 border-b border-aurora-border-default p-[var(--space-5)] pr-12 text-left">
        <div className="flex min-w-0 items-center gap-[var(--space-4)]">
          <Layers3 aria-hidden="true" className="size-5 shrink-0 text-aurora-accent-primary" />
          <div className="min-w-0">
            <DialogTitle className={`${AURORA_DISPLAY_2} break-words text-aurora-text-primary`}>{title}</DialogTitle>
            <DialogDescription className="mt-[var(--space-2)] text-xs text-aurora-text-muted">
              {artifact ? `${artifact.namespace ?? artifact.descriptor?.namespace ?? 'Publisher not supplied'} · ${artifact.providerId}` : 'Inspect the selected artifact and its source revision.'}
            </DialogDescription>
          </div>
        </div>
      </DialogHeader>
      {loading ? <div role="status" className="flex min-h-56 items-center justify-center text-sm text-aurora-text-muted"><Loader2 aria-hidden="true" className="mr-[var(--space-2)] size-4 animate-spin" />Loading artifact…</div>
        : artifact ? <>
          <div className="aurora-scrollbar min-h-0 flex-1 space-y-[var(--space-5)] overflow-y-auto p-[var(--space-5)]">
            <p className="text-sm leading-[var(--lh-body)]">{artifact.description ?? artifact.descriptor?.description ?? 'No description supplied by this source.'}</p>
            <div className="flex flex-wrap gap-[var(--space-2)] rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-[var(--space-3)]">
              <Badge variant="outline">{artifactKind(artifact)}</Badge>
              {artifact.publication?.state ? <Badge variant="outline">{artifact.publication.state}</Badge> : null}
              {artifact.publication?.visibility ? <Badge variant="outline">{artifact.publication.visibility}</Badge> : null}
              {artifact.currentRevision?.authoredAt ? <span className="self-center text-xs text-aurora-text-muted">Revision authored {artifact.currentRevision.authoredAt}</span> : null}
            </div>
            <section className="rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-medium">
              <h3 className={`${AURORA_MUTED_LABEL} border-b border-aurora-border-default p-[var(--space-4)]`}>Source and revision</h3>
              <div className="space-y-[var(--space-4)] p-[var(--space-4)]">
                {values.map(([label, value]) => value ? <div key={label} className="flex min-w-0 items-center gap-[var(--space-2)]">
                  <div className="min-w-0 flex-1"><p className={AURORA_MUTED_LABEL}>{label}</p><code className="mt-[var(--space-1)] block break-all text-xs">{value}</code></div>
                  <Button variant="ghost" size="sm" aria-label={`Copy ${label}`} onClick={() => onCopy(label, value)}>
                    {copied === label ? <Check aria-hidden="true" className="size-4 text-aurora-success" /> : <Copy aria-hidden="true" className="size-4" />}Copy
                  </Button>
                </div> : null)}
              </div>
            </section>
            <dl className="grid grid-cols-2 gap-[var(--space-4)] text-xs">
              {([['Declared license', artifact.license?.declared], ['License review', artifact.license?.reviewState], ['Distribution', artifact.publication?.distribution], ['Redistribution', artifact.license?.redistribution], ['Revisions', artifact.revisionCount?.toString()]] as const).map(([label, value]) =>
                <div key={label} className="min-w-0"><dt className={AURORA_MUTED_LABEL}>{label}</dt><dd className="mt-[var(--space-2)] break-words text-aurora-text-muted">{value || 'Not supplied'}</dd></div>
              )}
            </dl>
          </div>
          <DialogFooter className="shrink-0 flex-wrap border-t border-aurora-border-default bg-aurora-control-surface p-[var(--space-4)]">
            <Button variant="outline" size="sm" onClick={() => onExport(artifact)}><Download aria-hidden="true" className="size-4" />Export metadata</Button>
            <Button variant="outline" size="sm" onClick={() => onCopy('Artifact link', window.location.href)}><Link2 aria-hidden="true" className="size-4" />Copy link</Button>
            <Button size="sm" disabled={importing || !(artifact.currentRevisionId || artifact.currentRevision?.id)} onClick={() => void onImport(artifact)}>
              {importing ? <Loader2 aria-hidden="true" className="size-4 animate-spin" /> : <PackagePlus aria-hidden="true" className="size-4" />}{importing ? 'Importing…' : 'Send to Labby'}
            </Button>
          </DialogFooter>
        </> : <p role="status" className="p-[var(--space-7)] text-sm text-aurora-text-muted">Artifact details are unavailable. Close this dialog and retry from the catalog.</p>}
    </DialogContent>
  </Dialog>
}
