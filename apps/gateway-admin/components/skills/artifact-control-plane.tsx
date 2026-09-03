'use client'

import { useCallback, useEffect, useState } from 'react'
import { Archive, Boxes, DatabaseZap, Eye, Loader2, Play, RefreshCw, Search, Trash2, Upload } from 'lucide-react'
import { toast } from 'sonner'

import { ActionConfirmationDialog } from '@/components/action-confirmation-dialog'
import { AURORA_DENSE_META, AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Textarea } from '@/components/ui/textarea'
import { controlPlaneAction, uploadArtifactBytes } from '@/lib/api/artifact-control-client'
import { cn, getErrorMessage } from '@/lib/utils'

type JsonObject = Record<string, unknown>
type Source = JsonObject & { enabled?: boolean; intervalSeconds?: number }
type UploadRecord = JsonObject & { filename?: string }
type DeleteTarget = { kind: 'source' | 'upload' | 'bundle'; id: string } | null

function rows(value: unknown, key: string): JsonObject[] {
  if (!value || typeof value !== 'object') return []
  const candidate = (value as JsonObject)[key]
  return Array.isArray(candidate) ? candidate.filter((item): item is JsonObject => Boolean(item && typeof item === 'object')) : []
}

function objectAt(value: unknown, key: string): JsonObject | null {
  if (!value || typeof value !== 'object') return null
  const candidate = (value as JsonObject)[key]
  return candidate && typeof candidate === 'object' && !Array.isArray(candidate) ? candidate as JsonObject : null
}

function itemId(item: JsonObject, ...keys: string[]) {
  for (const key of keys) if (typeof item[key] === 'string') return item[key] as string
  return 'unknown'
}

export function ArtifactControlPlane() {
  const [sources, setSources] = useState<Source[]>([])
  const [jobs, setJobs] = useState<JsonObject[]>([])
  const [bundles, setBundles] = useState<JsonObject[]>([])
  const [uploads, setUploads] = useState<UploadRecord[]>([])
  const [results, setResults] = useState<JsonObject[]>([])
  const [authorityStatus, setAuthorityStatus] = useState<JsonObject | null>(null)
  const [selectedDetail, setSelectedDetail] = useState<JsonObject | null>(null)
  const [query, setQuery] = useState('')
  const [discoveryProvider, setDiscoveryProvider] = useState('authority')
  const [discoverySource, setDiscoverySource] = useState('')
  const [repoUrl, setRepoUrl] = useState('')
  const [namespace, setNamespace] = useState('imports')
  const [sourceId, setSourceId] = useState('')
  const [sourceInterval, setSourceInterval] = useState('3600')
  const [bundleSlug, setBundleSlug] = useState('')
  const [bundleDescription, setBundleDescription] = useState('')
  const [bundleVisibility, setBundleVisibility] = useState('oauth')
  const [memberNamespace, setMemberNamespace] = useState('')
  const [memberName, setMemberName] = useState('')
  const [artifactId, setArtifactId] = useState('')
  const [upstreamArtifactId, setUpstreamArtifactId] = useState('')
  const [forkNamespace, setForkNamespace] = useState('')
  const [forkName, setForkName] = useState('')
  const [publicationState, setPublicationState] = useState('draft')
  const [publicationVisibility, setPublicationVisibility] = useState('private')
  const [publicationDistribution, setPublicationDistribution] = useState('metadata')
  const [declaredLicense, setDeclaredLicense] = useState('')
  const [redistribution, setRedistribution] = useState('unknown')
  const [reviewState, setReviewState] = useState('unreviewed')
  const [takedownState, setTakedownState] = useState('none')
  const [candidateJson, setCandidateJson] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<DeleteTarget>(null)

  const refresh = useCallback(async (signal?: AbortSignal) => {
    setError(null)
    const settled = await Promise.allSettled([
      controlPlaneAction<JsonObject>('sources', 'sources.list', {}, signal),
      controlPlaneAction<JsonObject>('jobs', 'jobs.list', { limit: 50 }, signal),
      controlPlaneAction<JsonObject>('bundles', 'bundles.list', {}, signal),
      controlPlaneAction<JsonObject>('artifacts', 'artifacts.authority_status', {}, signal),
    ])
    if (signal?.aborted) return
    if (settled[0].status === 'fulfilled') setSources(rows(settled[0].value, 'sources'))
    if (settled[1].status === 'fulfilled') setJobs(rows(settled[1].value, 'jobs'))
    if (settled[2].status === 'fulfilled') setBundles(rows(settled[2].value, 'bundles'))
    if (settled[3].status === 'fulfilled') setAuthorityStatus(settled[3].value)
    const failure = settled.find((item): item is PromiseRejectedResult => item.status === 'rejected')
    if (failure) setError(getErrorMessage(failure.reason, 'Remote Artifact authority is unavailable.'))
  }, [])

  useEffect(() => {
    const controller = new AbortController()
    void refresh(controller.signal)
    return () => controller.abort()
  }, [refresh])

  async function run(label: string, operation: () => Promise<unknown>, success: string) {
    setBusy(label)
    try {
      const response = await operation()
      toast.success(success)
      await refresh()
      return response
    } catch (cause) {
      toast.error(getErrorMessage(cause, `Unable to ${label}.`))
      return null
    } finally {
      setBusy(null)
    }
  }

  async function inspect(service: 'artifacts' | 'jobs' | 'uploads' | 'bundles', action: string, params: object) {
    const response = await run('load details', () => controlPlaneAction<JsonObject>(service, action, params), 'Details loaded')
    if (response) setSelectedDetail(response as JsonObject)
  }

  async function searchRemote() {
    if (!['acp', 'mcp', 'marketplace', 'authority_all', 'candidates'].includes(discoveryProvider) && !query.trim()) return
    setBusy('search')
    try {
      const selection: Record<string, { action: string; params: JsonObject; key: string }> = {
        authority: { action: 'artifacts.search_remote', params: { query: query.trim(), limit: 50 }, key: 'results' },
        authority_all: { action: 'artifacts.list_remote', params: { limit: 50 }, key: 'artifacts' },
        candidates: { action: 'artifacts.list_candidates', params: { limit: 50 }, key: 'candidates' },
        skills_sh: { action: 'artifacts.search_skills_sh', params: { query: query.trim(), limit: 50 }, key: 'results' },
        ard: { action: 'artifacts.search_ard', params: { registry: discoverySource.trim(), query: query.trim() }, key: 'results' },
        marketplace: { action: 'artifacts.search_marketplace', params: { source: discoverySource.trim() }, key: 'plugins' },
        mcp: { action: 'artifacts.list_mcp_registry', params: { ...(query.trim() ? { query: query.trim() } : {}), limit: 50 }, key: 'servers' },
        acp: { action: 'artifacts.list_acp_registry', params: {}, key: 'agents' },
      }
      const selected = selection[discoveryProvider]
      if (!selected || (['ard', 'marketplace'].includes(discoveryProvider) && !discoverySource.trim())) return
      const response = await controlPlaneAction<JsonObject>('artifacts', selected.action, selected.params)
      const discovered = rows(response, selected.key)
      setResults(discovered.length ? discovered : rows(response, 'results'))
      setSelectedDetail(response)
    } catch (cause) {
      toast.error(getErrorMessage(cause, 'Unable to search the remote catalog.'))
    } finally { setBusy(null) }
  }

  async function submitCandidate() {
    let candidate: unknown
    try { candidate = JSON.parse(candidateJson) } catch { toast.error('Candidate must be valid JSON'); return }
    const response = await run('intake candidate', () => controlPlaneAction<JsonObject>('artifacts', 'artifacts.intake_candidate', { candidate }), 'Candidate evidence stored')
    if (response) setSelectedDetail(response as JsonObject)
  }

  async function upload(file?: File) {
    if (!file) return
    await run('upload Artifact', async () => {
      const created = await controlPlaneAction<JsonObject>('uploads', 'uploads.create', { filename: file.name })
      const uploadRecord = objectAt(created, 'upload')
      const uploadId = uploadRecord ? itemId(uploadRecord, 'id', 'uploadId') : ''
      if (!uploadId || uploadId === 'unknown') throw new Error('Authority did not return an upload id')
      setUploads(current => [{ ...uploadRecord, id: uploadId, filename: file.name }, ...current])
      await uploadArtifactBytes(uploadId, file)
      const job = await controlPlaneAction<JsonObject>('jobs', 'jobs.start', {
        kind: file.name.endsWith('.json') ? 'marketplace' : 'archive',
        arguments: file.name.endsWith('.json') ? { uploadId, baseSource: file.name } : { uploadId, namespace },
        idempotency_key: `gateway-admin-upload-${crypto.randomUUID()}`,
      })
      setSelectedDetail(job)
    }, 'Upload stored and ingestion queued')
  }

  async function configureSource(id: string, enabled: boolean, intervalSeconds?: number) {
    await run('configure source', () => controlPlaneAction('sources', 'sources.configure', {
      id, enabled, ...(intervalSeconds ? { interval_seconds: intervalSeconds } : {}),
    }), 'Source configuration saved')
  }

  async function mutateBundle(action: 'bundles.add' | 'bundles.remove') {
    if (!bundleSlug.trim() || !memberNamespace.trim() || !memberName.trim()) return
    await run(action === 'bundles.add' ? 'add bundle Artifact' : 'remove bundle Artifact', () => controlPlaneAction('bundles', action, {
      slug: bundleSlug.trim(), namespace: memberNamespace.trim(), name: memberName.trim(),
    }), action === 'bundles.add' ? 'Artifact added to bundle draft' : 'Artifact removed from bundle draft')
  }

  async function confirmDelete() {
    const target = deleteTarget
    if (!target) return
    const service = target.kind === 'source' ? 'sources' : target.kind === 'upload' ? 'uploads' : 'bundles'
    const params = target.kind === 'bundle' ? { slug: target.id } : { id: target.id }
    const response = await run(`delete ${target.kind}`, () => controlPlaneAction(service, `${service}.delete`, params), `${target.kind} deleted`)
    if (response && target.kind === 'upload') setUploads(current => current.filter(item => itemId(item, 'id', 'uploadId') !== target.id))
    if (response) setDeleteTarget(null)
  }

  return <div className="grid gap-4">
    <div className="flex flex-wrap items-start justify-between gap-3">
      <div><h2 className="font-display text-xl font-semibold">Artifact Control Plane</h2><p className={cn(AURORA_DENSE_META, 'mt-1 text-aurora-text-muted')}>Search, ingest, and operate durable sources, jobs, uploads, and bundles through Labby.</p></div>
      <Button variant="outline" size="sm" onClick={() => void refresh()} disabled={busy !== null}><RefreshCw className="size-4" />Refresh state</Button>
    </div>
    {error ? <DashboardPanel title="Authority status"><p className="text-sm text-destructive">{error}</p></DashboardPanel> : authorityStatus ? <DashboardPanel title="Authority status"><pre className="overflow-auto whitespace-pre-wrap text-xs text-aurora-text-secondary">{JSON.stringify(authorityStatus, null, 2)}</pre></DashboardPanel> : null}
    <Tabs defaultValue="discover">
      <TabsList aria-label="Artifact control-plane views"><TabsTrigger value="discover">Discover & ingest</TabsTrigger><TabsTrigger value="governance">Lifecycle</TabsTrigger><TabsTrigger value="jobs">Jobs</TabsTrigger><TabsTrigger value="sources">Sources</TabsTrigger><TabsTrigger value="uploads">Uploads</TabsTrigger><TabsTrigger value="bundles">Bundles</TabsTrigger></TabsList>

      <TabsContent value="discover" className="mt-3 grid gap-3 lg:grid-cols-2">
        <DashboardPanel title="Remote search" icon={<Search className="size-4" />}>
          <form className="grid gap-2" onSubmit={event => { event.preventDefault(); void searchRemote() }}>
            <Select value={discoveryProvider} onValueChange={setDiscoveryProvider}><SelectTrigger aria-label="Discovery catalog"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="authority">Search Artifact authority</SelectItem><SelectItem value="authority_all">All authority Artifacts</SelectItem><SelectItem value="candidates">Intake candidates</SelectItem><SelectItem value="skills_sh">skills.sh</SelectItem><SelectItem value="ard">ARD registry</SelectItem><SelectItem value="marketplace">Plugin marketplace</SelectItem><SelectItem value="mcp">MCP Registry</SelectItem><SelectItem value="acp">ACP Registry</SelectItem></SelectContent></Select>
            {['ard', 'marketplace'].includes(discoveryProvider) ? <Input aria-label="Discovery source" value={discoverySource} onChange={event => setDiscoverySource(event.target.value)} placeholder={discoveryProvider === 'ard' ? 'ARD registry URL or domain' : 'Marketplace repository or manifest URL'} /> : null}
            <div className="flex gap-2">{!['acp', 'marketplace', 'authority_all', 'candidates'].includes(discoveryProvider) ? <Input aria-label="Search remote Artifact catalog" value={query} onChange={event => setQuery(event.target.value)} placeholder="Search names, namespaces, descriptions" /> : null}<Button type="submit" disabled={busy !== null || (['ard', 'marketplace'].includes(discoveryProvider) && !discoverySource.trim()) || (!['acp', 'mcp', 'marketplace', 'authority_all', 'candidates'].includes(discoveryProvider) && !query.trim())}>{busy === 'search' ? <Loader2 className="size-4 animate-spin" /> : <Search className="size-4" />}{['mcp', 'acp', 'authority_all', 'candidates'].includes(discoveryProvider) ? 'List' : 'Search'}</Button></div>
          </form>
          <div className="grid gap-2">{results.map((item, index) => { const rowId = itemId(item, 'id', 'artifactId', 'uri', 'name'); const inspectId = itemId(item, 'id', 'artifactId'); return <div key={`${rowId}-${index}`} className="rounded-aurora-1 border border-aurora-border-subtle bg-aurora-control-surface p-3"><div className="flex items-center justify-between gap-2"><span className="text-sm font-medium">{itemId(item, 'name', 'displayName', 'artifactId', 'uri')}</span>{typeof item.namespace === 'string' ? <Badge variant="outline">{item.namespace}</Badge> : null}</div>{typeof item.description === 'string' ? <p className={cn(AURORA_DENSE_META, 'mt-1 text-aurora-text-muted')}>{item.description}</p> : null}{inspectId !== 'unknown' ? <Button className="mt-2" size="sm" variant="ghost" onClick={() => { setArtifactId(inspectId); void inspect('artifacts', 'artifacts.get_remote', { id: inspectId }) }}><Eye className="size-4" />Inspect</Button> : null}</div> })}</div>
        </DashboardPanel>
        <DashboardPanel title="Ingest" icon={<DatabaseZap className="size-4" />}>
          <label className="grid gap-1"><span className={AURORA_MUTED_LABEL}>Git repository URL</span><Input value={repoUrl} onChange={event => setRepoUrl(event.target.value)} placeholder="https://github.com/unraid/limetech-ai-skills" /></label>
          <label className="grid gap-1"><span className={AURORA_MUTED_LABEL}>Namespace</span><Input value={namespace} onChange={event => setNamespace(event.target.value)} /></label>
          <Button disabled={!repoUrl.trim() || busy !== null} onClick={() => void run('ingest repository', () => controlPlaneAction('jobs', 'jobs.start', { kind: 'repo', arguments: { url: repoUrl.trim(), namespace }, idempotency_key: `gateway-admin-repo-${crypto.randomUUID()}` }), 'Repository ingestion queued')}><Play className="size-4" />Ingest repository</Button>
          <div className="border-t border-aurora-border-subtle pt-3"><label className="flex cursor-pointer items-center justify-center gap-2 rounded-aurora-1 border border-dashed border-aurora-border-strong px-4 py-6 text-sm hover:bg-aurora-hover-bg"><Upload className="size-4" />Upload archive or marketplace JSON<input className="sr-only" type="file" accept=".zip,.tar,.gz,.tgz,.json" onChange={event => { const file = event.target.files?.[0]; void upload(file); event.currentTarget.value = '' }} /></label></div>
        </DashboardPanel>
      </TabsContent>

      <TabsContent value="governance" className="mt-3 grid gap-3 xl:grid-cols-2">
        <DashboardPanel title="Artifact lifecycle" icon={<RefreshCw className="size-4" />}>
          <label className="grid gap-1"><span className={AURORA_MUTED_LABEL}>Artifact ID</span><Input value={artifactId} onChange={event => setArtifactId(event.target.value)} /></label>
          <div className="grid gap-2 rounded-aurora-1 border border-aurora-border-subtle p-3"><p className={AURORA_MUTED_LABEL}>Follow upstream</p><Input aria-label="Upstream Artifact ID" value={upstreamArtifactId} onChange={event => setUpstreamArtifactId(event.target.value)} placeholder="Upstream Artifact ID" /><div className="flex gap-2"><Button size="sm" disabled={!artifactId || !upstreamArtifactId || busy !== null} onClick={() => void run('follow Artifact', () => controlPlaneAction('artifacts', 'artifacts.follow', { id: artifactId, upstream_artifact_id: upstreamArtifactId, following: true }), 'Artifact now follows upstream')}>Follow</Button><Button size="sm" variant="outline" disabled={!artifactId || busy !== null} onClick={() => void run('unfollow Artifact', () => controlPlaneAction('artifacts', 'artifacts.follow', { id: artifactId, following: false }), 'Artifact detached from upstream')}>Unfollow</Button></div></div>
          <div className="grid gap-2 rounded-aurora-1 border border-aurora-border-subtle p-3"><p className={AURORA_MUTED_LABEL}>Fork exact Artifact</p><div className="grid grid-cols-2 gap-2"><Input aria-label="Fork namespace" value={forkNamespace} onChange={event => setForkNamespace(event.target.value)} placeholder="namespace" /><Input aria-label="Fork name" value={forkName} onChange={event => setForkName(event.target.value)} placeholder="name" /></div><Button size="sm" disabled={!artifactId || !forkNamespace || !forkName || busy !== null} onClick={() => void run('fork Artifact', async () => { const response = await controlPlaneAction<JsonObject>('artifacts', 'artifacts.fork', { source_artifact_id: artifactId, namespace: forkNamespace, name: forkName, following: false }); setSelectedDetail(response); return response }, 'Artifact fork created')}>Fork</Button></div>
        </DashboardPanel>
        <DashboardPanel title="Publication and license">
          <div className="grid grid-cols-3 gap-2"><Select value={publicationState} onValueChange={setPublicationState}><SelectTrigger aria-label="Publication state"><SelectValue /></SelectTrigger><SelectContent>{['draft', 'listed', 'published', 'withdrawn'].map(value => <SelectItem key={value} value={value}>{value}</SelectItem>)}</SelectContent></Select><Select value={publicationVisibility} onValueChange={setPublicationVisibility}><SelectTrigger aria-label="Publication visibility"><SelectValue /></SelectTrigger><SelectContent>{['private', 'unlisted', 'public'].map(value => <SelectItem key={value} value={value}>{value}</SelectItem>)}</SelectContent></Select><Select value={publicationDistribution} onValueChange={setPublicationDistribution}><SelectTrigger aria-label="Distribution mode"><SelectValue /></SelectTrigger><SelectContent>{['metadata', 'bytes'].map(value => <SelectItem key={value} value={value}>{value}</SelectItem>)}</SelectContent></Select></div>
          <Button size="sm" disabled={!artifactId || busy !== null} onClick={() => void run('set publication', () => controlPlaneAction('artifacts', 'artifacts.set_publication', { id: artifactId, state: publicationState, visibility: publicationVisibility, distribution: publicationDistribution }), 'Publication policy saved')}>Save publication</Button>
          <div className="grid gap-2 border-t border-aurora-border-subtle pt-3"><Input aria-label="Declared license" value={declaredLicense} onChange={event => setDeclaredLicense(event.target.value)} placeholder="Declared license, for example MIT" /><div className="grid grid-cols-3 gap-2"><Select value={redistribution} onValueChange={setRedistribution}><SelectTrigger aria-label="Redistribution"><SelectValue /></SelectTrigger><SelectContent>{['metadata_only', 'cache_for_index', 'redistributable', 'forkable', 'restricted', 'unknown'].map(value => <SelectItem key={value} value={value}>{value}</SelectItem>)}</SelectContent></Select><Select value={reviewState} onValueChange={setReviewState}><SelectTrigger aria-label="Review state"><SelectValue /></SelectTrigger><SelectContent>{['unreviewed', 'reviewed', 'disputed'].map(value => <SelectItem key={value} value={value}>{value}</SelectItem>)}</SelectContent></Select><Select value={takedownState} onValueChange={setTakedownState}><SelectTrigger aria-label="Takedown state"><SelectValue /></SelectTrigger><SelectContent>{['none', 'requested', 'restricted', 'removed'].map(value => <SelectItem key={value} value={value}>{value}</SelectItem>)}</SelectContent></Select></div><Button size="sm" disabled={!artifactId || busy !== null} onClick={() => void run('set license policy', () => controlPlaneAction('artifacts', 'artifacts.set_license', { id: artifactId, declared: declaredLicense || null, redistribution, review_state: reviewState, takedown_state: takedownState }), 'License policy saved')}>Save license policy</Button></div>
        </DashboardPanel>
        <DashboardPanel title="Candidate intake"><Textarea aria-label="Artifact candidate JSON" value={candidateJson} onChange={event => setCandidateJson(event.target.value)} placeholder='{"schema":"dinglebear.artifact-candidate/v1", ...}' rows={8} /><Button size="sm" disabled={!candidateJson.trim() || busy !== null} onClick={() => void submitCandidate()}>Intake candidate</Button></DashboardPanel>
        <DashboardPanel title="Inspector"><pre className="max-h-[420px] overflow-auto whitespace-pre-wrap text-xs text-aurora-text-secondary">{selectedDetail ? JSON.stringify(selectedDetail, null, 2) : 'Lifecycle responses and candidate evidence appear here.'}</pre></DashboardPanel>
      </TabsContent>

      <TabsContent value="jobs" className="mt-3 grid gap-3 lg:grid-cols-[minmax(0,1fr)_minmax(280px,0.7fr)]">
        <DashboardPanel title="Durable ingestion jobs" icon={<DatabaseZap className="size-4" />} meta={`${jobs.length}`}><div className="grid gap-2">{jobs.length ? jobs.map(job => { const id = itemId(job, 'id', 'jobId'); const status = itemId(job, 'status'); return <div key={id} className="flex flex-wrap items-center gap-2 rounded-aurora-1 border border-aurora-border-subtle p-3"><button className="min-w-0 flex-1 text-left" onClick={() => void inspect('jobs', 'jobs.get', { id })}><p className="truncate text-sm font-medium">{itemId(job, 'kind', 'operation')} · {id}</p><p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>{typeof job.progress === 'string' ? job.progress : status}</p></button><Badge variant="outline">{status}</Badge>{['queued', 'running'].includes(status) ? <Button size="sm" variant="outline" onClick={() => void run('cancel job', () => controlPlaneAction('jobs', 'jobs.cancel', { id }), 'Cancellation requested')}>Cancel</Button> : <Button size="sm" variant="outline" onClick={() => void run('retry job', () => controlPlaneAction('jobs', 'jobs.retry', { id }), 'Retry queued')}>Retry</Button>}</div> }) : <p className="text-sm text-aurora-text-muted">No ingestion jobs yet.</p>}</div></DashboardPanel>
        <DashboardPanel title="Inspector"><pre className="max-h-[420px] overflow-auto whitespace-pre-wrap text-xs text-aurora-text-secondary">{selectedDetail ? JSON.stringify(selectedDetail, null, 2) : 'Select a job or Artifact to inspect its projected metadata.'}</pre></DashboardPanel>
      </TabsContent>

      <TabsContent value="sources" className="mt-3 grid gap-3 lg:grid-cols-[minmax(0,1fr)_320px]">
        <DashboardPanel title="Refreshable sources" icon={<Archive className="size-4" />} meta={`${sources.length}`}><div className="grid gap-2">{sources.length ? sources.map(source => { const id = itemId(source, 'id', 'sourceId'); const enabled = source.enabled !== false; return <div key={id} className="flex flex-wrap items-center gap-2 rounded-aurora-1 border border-aurora-border-subtle p-3"><div className="min-w-0 flex-1"><p className="truncate text-sm font-medium">{id}</p><p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>{enabled ? itemId(source, 'status', 'kind') : 'Paused'} · {source.intervalSeconds ?? 'default'}s</p></div><Button size="sm" variant="outline" onClick={() => void configureSource(id, !enabled, source.intervalSeconds)}>{enabled ? 'Pause' : 'Enable'}</Button><Button size="sm" variant="outline" onClick={() => void run('refresh source', () => controlPlaneAction('sources', 'sources.refresh', { id }), 'Source refresh queued')}>Refresh</Button><Button aria-label={`Delete source ${id}`} size="icon-sm" variant="ghost" onClick={() => setDeleteTarget({ kind: 'source', id })}><Trash2 className="size-4" /></Button></div> }) : <p className="text-sm text-aurora-text-muted">No persisted sources yet.</p>}</div></DashboardPanel>
        <DashboardPanel title="Configure source"><label className="grid gap-1"><span className={AURORA_MUTED_LABEL}>Source ID</span><Input value={sourceId} onChange={event => setSourceId(event.target.value)} /></label><label className="grid gap-1"><span className={AURORA_MUTED_LABEL}>Refresh interval (seconds)</span><Input type="number" min={60} value={sourceInterval} onChange={event => setSourceInterval(event.target.value)} /></label><Button disabled={!sourceId.trim() || !Number(sourceInterval) || busy !== null} onClick={() => void configureSource(sourceId.trim(), true, Number(sourceInterval))}>Save schedule</Button></DashboardPanel>
      </TabsContent>

      <TabsContent value="uploads" className="mt-3"><DashboardPanel title="Current-session uploads" icon={<Upload className="size-4" />} meta={`${uploads.length}`}><div className="grid gap-2">{uploads.length ? uploads.map(uploadRecord => { const id = itemId(uploadRecord, 'id', 'uploadId'); return <div key={id} className="flex items-center gap-2 rounded-aurora-1 border border-aurora-border-subtle p-3"><button className="min-w-0 flex-1 text-left" onClick={() => void inspect('uploads', 'uploads.get', { id })}><p className="truncate text-sm font-medium">{itemId(uploadRecord, 'filename')} · {id}</p><p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>{itemId(uploadRecord, 'status')}</p></button><Button aria-label={`Delete upload ${id}`} size="icon-sm" variant="ghost" onClick={() => setDeleteTarget({ kind: 'upload', id })}><Trash2 className="size-4" /></Button></div> }) : <p className="text-sm text-aurora-text-muted">Uploads created in this browser session appear here until ingested or deleted.</p>}</div></DashboardPanel></TabsContent>

      <TabsContent value="bundles" className="mt-3 grid gap-3 lg:grid-cols-[minmax(0,1fr)_360px]">
        <DashboardPanel title="Curated bundles" icon={<Boxes className="size-4" />} meta={`${bundles.length}`}><div className="grid gap-2">{bundles.length ? bundles.map(bundle => { const slug = itemId(bundle, 'slug'); return <div key={slug} className="flex flex-wrap items-center gap-2 rounded-aurora-1 border border-aurora-border-subtle p-3"><button className="min-w-0 flex-1 text-left" onClick={() => { setBundleSlug(slug); void inspect('bundles', 'bundles.get', { slug }) }}><p className="text-sm font-medium">{slug}</p><p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>{typeof bundle.description === 'string' ? bundle.description : `${Array.isArray(bundle.members) ? bundle.members.length : bundle.members ?? 0} members`}</p></button><Badge variant="outline">{typeof bundle.visibility === 'string' ? bundle.visibility : 'private'}</Badge><Button size="sm" variant="outline" onClick={() => void run('publish bundle', () => controlPlaneAction('bundles', 'bundles.publish', { slug }), 'Bundle published')}>Publish</Button><Button aria-label={`Delete bundle ${slug}`} size="icon-sm" variant="ghost" onClick={() => setDeleteTarget({ kind: 'bundle', id: slug })}><Trash2 className="size-4" /></Button></div> }) : <p className="text-sm text-aurora-text-muted">No bundles yet.</p>}</div></DashboardPanel>
        <DashboardPanel title="Bundle editor"><label className="grid gap-1"><span className={AURORA_MUTED_LABEL}>Slug</span><Input value={bundleSlug} onChange={event => setBundleSlug(event.target.value)} placeholder="starter-pack" /></label><label className="grid gap-1"><span className={AURORA_MUTED_LABEL}>Description</span><Input value={bundleDescription} onChange={event => setBundleDescription(event.target.value)} /></label><Select value={bundleVisibility} onValueChange={setBundleVisibility}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="public">Public</SelectItem><SelectItem value="bearer">Bearer</SelectItem><SelectItem value="oauth">OAuth</SelectItem></SelectContent></Select><div className="flex gap-2"><Button disabled={!bundleSlug.trim() || busy !== null} onClick={() => void run('create bundle', () => controlPlaneAction('bundles', 'bundles.create', { slug: bundleSlug.trim(), description: bundleDescription.trim() || undefined, visibility: bundleVisibility }), 'Bundle created')}><Boxes className="size-4" />Create</Button><Button variant="outline" disabled={!bundleSlug.trim() || busy !== null} onClick={() => void run('set bundle visibility', () => controlPlaneAction('bundles', 'bundles.set_visibility', { slug: bundleSlug.trim(), visibility: bundleVisibility }), 'Visibility saved')}>Save visibility</Button></div><div className="grid gap-2 border-t border-aurora-border-subtle pt-3"><p className={AURORA_MUTED_LABEL}>Draft membership</p><div className="grid grid-cols-2 gap-2"><Input aria-label="Artifact namespace" value={memberNamespace} onChange={event => setMemberNamespace(event.target.value)} placeholder="namespace" /><Input aria-label="Artifact name" value={memberName} onChange={event => setMemberName(event.target.value)} placeholder="artifact" /></div><div className="flex gap-2"><Button size="sm" disabled={!bundleSlug || !memberNamespace || !memberName || busy !== null} onClick={() => void mutateBundle('bundles.add')}>Add Artifact</Button><Button size="sm" variant="outline" disabled={!bundleSlug || !memberNamespace || !memberName || busy !== null} onClick={() => void mutateBundle('bundles.remove')}>Remove Artifact</Button></div></div></DashboardPanel>
      </TabsContent>
    </Tabs>
    <ActionConfirmationDialog open={deleteTarget !== null} onOpenChange={open => { if (!open) setDeleteTarget(null) }} title={`Delete ${deleteTarget?.kind ?? 'item'}?`} description={deleteTarget?.kind === 'bundle' ? 'This permanently removes the bundle and its published manifests. Stored Artifacts are not deleted.' : `This removes ${deleteTarget?.id ?? 'the selected item'} from the remote authority.`} confirmLabel="Delete" busy={busy?.startsWith('delete') ?? false} onConfirm={() => void confirmDelete()} />
  </div>
}
