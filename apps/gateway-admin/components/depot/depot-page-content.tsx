'use client'

import { useCallback, useEffect, useMemo, useState } from 'react'
import { useSearchParams } from 'next/navigation'
import { Archive, Box, Loader2, RefreshCw, Search, ShieldCheck, Upload } from 'lucide-react'
import { toast } from 'sonner'

import { AppHeader } from '@/components/app-header'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { depotCall, depotSession, depotStatus, depotUpload, importDepotSkill, type DepotArtifact, type DepotStatus } from '@/lib/api/depot-client'

type LoadState = { loading: boolean; error?: string; status?: DepotStatus; session?: Record<string, unknown>; artifacts: DepotArtifact[]; cursor?: string }

export function DepotPageContent() {
  const searchParams = useSearchParams()
  const selectedId = searchParams.get('artifact')?.trim()
  const [query, setQuery] = useState('')
  const [state, setState] = useState<LoadState>({ loading: true, artifacts: [] })
  const [detail, setDetail] = useState<DepotArtifact | null>(null)
  const [file, setFile] = useState<File | null>(null)
  const [jobs, setJobs] = useState<Array<Record<string, unknown>>>([])

  const load = useCallback(async (cursor?: string, signal?: AbortSignal) => {
    setState((current) => ({ ...current, loading: true, error: undefined }))
    try {
      const [status, session, listing] = await Promise.all([
        depotStatus(signal), depotSession(signal),
        depotCall<{ result?: { artifacts?: DepotArtifact[]; nextCursor?: string } }>('depot.artifacts.list', { limit: 50, ...(cursor ? { cursor } : {}) }, signal),
      ])
      setState({ loading: false, status, session, artifacts: listing.result?.artifacts ?? [], cursor: listing.result?.nextCursor })
    } catch (error) {
      if (signal?.aborted) return
      setState((current) => ({ ...current, loading: false, error: error instanceof Error ? error.message : String(error) }))
    }
  }, [])

  useEffect(() => { const controller = new AbortController(); void load(undefined, controller.signal); return () => controller.abort() }, [load])
  useEffect(() => {
    if (!selectedId) { setDetail(null); return }
    const controller = new AbortController()
    void depotCall<{ result?: { artifact?: DepotArtifact } }>('depot.artifacts.get', { artifactId: selectedId }, controller.signal)
      .then((response) => setDetail(response.result?.artifact ?? null))
      .catch((error) => { if (!controller.signal.aborted) toast.error(error instanceof Error ? error.message : String(error)) })
    return () => controller.abort()
  }, [selectedId])

  const filtered = useMemo(() => state.artifacts.filter((artifact) => JSON.stringify(artifact.descriptor ?? {}).toLowerCase().includes(query.toLowerCase())), [state.artifacts, query])
  const mutate = async (operation: string, params: Record<string, unknown>, message: string) => {
    try { await depotCall(operation, params); toast.success(message); await load() }
    catch (error) { toast.error(error instanceof Error ? error.message : String(error)) }
  }
  const ingestArchive = async () => {
    if (!file) return
    try {
      const slot = await depotCall<{ result?: { upload?: { id?: string; uploadId?: string } } }>('depot.uploads.create', { filename: file.name })
      const uploadId = slot.result?.upload?.id ?? slot.result?.upload?.uploadId
      if (!uploadId) throw new Error('Depot did not return an upload id')
      await depotUpload(uploadId, file)
      await depotCall('depot.ingest.start', { kind: 'archive', arguments: { uploadId }, idempotencyKey: `labby-${uploadId}` })
      toast.success('Archive ingest started')
      setFile(null)
      const response = await depotCall<{ result?: { jobs?: Array<Record<string, unknown>> } }>('depot.ingest.list', { limit: 20 })
      setJobs(response.result?.jobs ?? [])
    } catch (error) { toast.error(error instanceof Error ? error.message : String(error)) }
  }

  return <>
    <AppHeader breadcrumbs={[{ label: 'Depot' }, ...(selectedId ? [{ label: selectedId }] : [])]} />
    <div className={`${AURORA_PAGE_SHELL} flex-1`}><div className={AURORA_PAGE_FRAME}>
      <ConsoleHero eyebrow="Unified control plane" title="Depot Bazaar" pulse={{ color: state.status?.enabled ? 'var(--aurora-success)' : 'var(--aurora-warn)', label: state.status?.enabled ? 'connected through Labby' : 'disabled' }} actions={<Button variant="outline" size="sm" onClick={() => void load()} disabled={state.loading}>{state.loading ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />}Refresh</Button>} stats={[
        { label: 'Artifacts', value: state.artifacts.length, icon: <Archive size={12}/> },
        { label: 'Authority', value: state.status?.mutationAuthority ? 'Delegated' : 'Read only', icon: <ShieldCheck size={12}/> },
        { label: 'Page limit', value: 50, icon: <Box size={12}/> },
      ]}/>
      {state.error ? <DashboardPanel title="Depot unavailable"><p role="alert" className="text-sm text-destructive">{state.error}. Labby-only routes remain available.</p></DashboardPanel> : null}
      <DashboardPanel title="Browse immutable artifacts" icon={<Search className="size-4"/>} action={<Input aria-label="Search Depot artifacts" className="h-8 w-64" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search name, kind, namespace"/>}>
        <div className="divide-y divide-aurora-border-subtle">
          {filtered.map((artifact) => { const id = artifact.descriptor?.id ?? 'unknown'; return <a key={id} href={`/depot?artifact=${encodeURIComponent(id)}`} className="flex min-h-14 items-center justify-between gap-4 rounded px-2 py-2 hover:bg-aurora-surface-muted focus-visible:outline-none focus-visible:ring-2"><div><div className="font-medium">{artifact.descriptor?.name ?? id}</div><div className="text-xs text-muted-foreground">{artifact.descriptor?.kind} · {artifact.descriptor?.namespace} · {id}</div></div><Badge variant="outline">{artifact.publication?.visibility ?? 'private'}</Badge></a> })}
          {!state.loading && filtered.length === 0 ? <p className="py-8 text-center text-sm text-muted-foreground">No matching artifacts.</p> : null}
        </div>
        {state.cursor ? <Button variant="outline" onClick={() => void load(state.cursor)}>Next page</Button> : null}
      </DashboardPanel>
      {detail && selectedId ? <DashboardPanel title="Artifact detail" icon={<Box className="size-4"/>}>
        <h2 className="text-lg font-semibold">{detail.descriptor?.name ?? selectedId}</h2><p className="text-sm text-muted-foreground">{detail.descriptor?.summary ?? 'No summary supplied.'}</p><code className="break-all text-xs">{detail.currentRevisionId}</code>
        <div className="flex flex-wrap gap-2">
          {detail.descriptor?.kind === 'skill' ? <Button onClick={() => void importDepotSkill({ source_id: 'depot', artifact_id: selectedId, revision_id: detail.currentRevisionId }).then(() => toast.success('Exact revision imported')).catch((error) => toast.error(String(error)))}><Upload className="size-4"/>Import exact revision</Button> : null}
          <Button variant="outline" onClick={() => void mutate('depot.artifacts.follow', { artifactId: selectedId, expectedRevision: detail.currentRevisionId, following: !detail.lineage?.following }, detail.lineage?.following ? 'Following disabled' : 'Following enabled')}>{detail.lineage?.following ? 'Unfollow' : 'Follow'}</Button>
          <Button variant="outline" onClick={() => void mutate('depot.artifacts.set_publication', { artifactId: selectedId, expectedRevision: detail.currentRevisionId, visibility: 'private' }, 'Visibility updated')}>Make private</Button>
        </div>
      </DashboardPanel> : null}
      <DashboardPanel title="Bounded archive ingest" icon={<Upload className="size-4"/>}>
        <p className="text-sm text-muted-foreground">Create a principal-owned upload slot, transfer at most 64 MiB through Labby, then start a durable Depot job with an idempotency key.</p>
        <div className="flex flex-wrap items-center gap-2"><Input aria-label="Archive to ingest" type="file" accept=".zip,.tar,.tgz,.json" onChange={(event) => setFile(event.target.files?.[0] ?? null)} /><Button disabled={!file || !state.status?.mutationAuthority} onClick={() => void ingestArchive()}><Upload className="size-4"/>Start ingest</Button><Button variant="outline" onClick={() => void depotCall<{ result?: { jobs?: Array<Record<string, unknown>> } }>('depot.ingest.list', { limit: 20 }).then((response) => setJobs(response.result?.jobs ?? [])).catch((error) => toast.error(String(error)))}>Refresh jobs</Button></div>
        {jobs.length ? <div className="divide-y divide-aurora-border-subtle">{jobs.map((job, index) => <div key={String(job.id ?? index)} className="flex items-center justify-between py-2 text-sm"><code>{String(job.id ?? 'job')}</code><Badge variant="outline">{String(job.status ?? job.state ?? 'unknown')}</Badge></div>)}</div> : null}
      </DashboardPanel>
    </div></div>
  </>
}
