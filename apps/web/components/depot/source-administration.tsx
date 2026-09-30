'use client'

import { useCallback, useEffect, useState } from 'react'
import { CirclePause, CirclePlay, GitBranch, Loader2, RefreshCw, RotateCcw, Trash2 } from 'lucide-react'

import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from '@/components/ui/alert-dialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import {
  cancelDepotIngestJob,
  configureDepotSource,
  deleteDepotSource,
  depotIngestJobs,
  depotSources,
  refreshDepotSource,
  retryDepotIngestJob,
  startDepotRepoIngest,
  type DepotIngestJob,
  type DepotSource,
} from '@/lib/api/depot-client'

function text(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value : undefined
}

function formatTimestamp(value?: string): string {
  if (!value) return 'Never'
  const date = new Date(value)
  return Number.isNaN(date.valueOf()) ? value : date.toLocaleString()
}

export function sourcePresentation(source: DepotSource): { label: string; location: string } {
  const location = text(source.args.url)
    ?? text(source.args.domain)
    ?? text(source.args.source)
    ?? text(source.args.endpoint)
    ?? text(source.args.registry)
    ?? source.id
  return {
    label: text(source.args.namespace) ?? source.kind.replaceAll('_', ' '),
    location,
  }
}

function SourceState({ source }: { source: DepotSource }) {
  if (!source.enabled) return <Badge variant="secondary">Paused</Badge>
  if ((source.consecutiveFailures ?? 0) > 0 || source.lastError) return <Badge variant="outline" className="border-destructive/40 bg-destructive/10 text-destructive">Needs attention</Badge>
  if (source.lastSuccessAt) return <Badge variant="outline">Healthy</Badge>
  return <Badge variant="secondary">Awaiting first refresh</Badge>
}

export function DepotSourceAdministration({ onOpenCatalog }: { onOpenCatalog?: () => void }) {
  const [sources, setSources] = useState<DepotSource[]>([])
  const [jobs, setJobs] = useState<DepotIngestJob[]>([])
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [jobsError, setJobsError] = useState<string | null>(null)
  const [deleteCandidate, setDeleteCandidate] = useState<DepotSource | null>(null)
  const [url, setUrl] = useState('')
  const [namespace, setNamespace] = useState('')
  const [ref, setRef] = useState('')
  const [subdir, setSubdir] = useState('')
  const [credential, setCredential] = useState('github-private')
  const [intervalDrafts, setIntervalDrafts] = useState<Record<string, string>>({})

  const load = useCallback(async (signal?: AbortSignal) => {
    setLoading(true)
    setError(null)
    try {
      const [sourcesResult, jobsResult] = await Promise.allSettled([
        depotSources(signal),
        depotIngestJobs(25, signal),
      ])
      if (sourcesResult.status === 'fulfilled') {
        setSources(sourcesResult.value)
        setIntervalDrafts(Object.fromEntries(sourcesResult.value.map(source => [source.id, String(source.intervalSeconds)])))
      } else if (!(sourcesResult.reason instanceof DOMException && sourcesResult.reason.name === 'AbortError')) {
        setError(sourcesResult.reason instanceof Error ? sourcesResult.reason.message : 'Unable to load Depot sources.')
      }
      if (jobsResult.status === 'fulfilled') {
        setJobs(jobsResult.value)
        setJobsError(null)
      } else if (!(jobsResult.reason instanceof DOMException && jobsResult.reason.name === 'AbortError')) {
        setJobsError(jobsResult.reason instanceof Error ? jobsResult.reason.message : 'Recent ingest jobs are unavailable.')
      }
    } finally {
      if (!signal?.aborted) setLoading(false)
    }
  }, [])

  useEffect(() => {
    const controller = new AbortController()
    void load(controller.signal)
    return () => controller.abort()
  }, [load])

  async function mutate(key: string, action: () => Promise<unknown>) {
    setBusy(key)
    setError(null)
    try {
      await action()
      await load()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Depot operation failed.')
    } finally {
      setBusy(null)
    }
  }

  async function addRepository() {
    if (!url.trim() || !namespace.trim()) {
      setError('Repository URL and namespace are required.')
      return
    }
    await mutate('add-repository', async () => {
      await startDepotRepoIngest({ url, namespace, ref, subdir, credential })
      setUrl('')
      setNamespace('')
      setRef('')
      setSubdir('')
    })
  }

  return (
    <div className="space-y-6">
      <div className="grid gap-6 xl:grid-cols-[minmax(0,2fr)_minmax(320px,1fr)]">
        <Card>
          <CardHeader>
            <CardTitle>Persisted sources</CardTitle>
            <CardDescription>
              Every durable refreshable Depot source, including repositories, registries, marketplaces, ARD, well-known catalogs, and Skills-over-MCP endpoints.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            {error ? <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">{error}</div> : null}
            <div className="overflow-x-auto rounded-lg border border-aurora-border-strong">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Source</TableHead>
                    <TableHead>State</TableHead>
                    <TableHead>Refresh</TableHead>
                    <TableHead>Revision</TableHead>
                    <TableHead>Last result</TableHead>
                    <TableHead className="text-right">Actions</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {loading ? (
                    <TableRow><TableCell colSpan={6} className="py-10 text-center text-aurora-text-muted"><Loader2 className="mr-2 inline size-4 animate-spin" />Loading Depot sources</TableCell></TableRow>
                  ) : sources.length === 0 ? (
                    <TableRow><TableCell colSpan={6} className="py-10 text-center text-aurora-text-muted">No persisted sources are configured.</TableCell></TableRow>
                  ) : sources.map(source => {
                    const presentation = sourcePresentation(source)
                    const rowBusy = busy?.endsWith(source.id) ?? false
                    const intervalDraft = intervalDrafts[source.id] ?? String(source.intervalSeconds)
                    const parsedInterval = Number(intervalDraft)
                    const intervalValid = Number.isInteger(parsedInterval) && parsedInterval >= 1 && parsedInterval <= 31_536_000
                    const intervalChanged = intervalValid && parsedInterval !== source.intervalSeconds
                    return (
                      <TableRow key={source.id}>
                        <TableCell className="max-w-[320px] align-top">
                          <div className="font-medium text-aurora-text-primary">{presentation.label}</div>
                          <div className="mt-1 break-all font-mono text-xs text-aurora-text-muted">{presentation.location}</div>
                          <div className="mt-1 flex flex-wrap gap-1">
                            <Badge variant="outline">{source.kind.replaceAll('_', ' ')}</Badge>
                            {text(source.args.ref) ? <Badge variant="outline">ref {text(source.args.ref)}</Badge> : null}
                            {text(source.args.subdir) ? <Badge variant="outline">{text(source.args.subdir)}</Badge> : null}
                            {text(source.args.credential) ? <Badge variant="secondary">credential {text(source.args.credential)}</Badge> : null}
                          </div>
                        </TableCell>
                        <TableCell className="align-top"><SourceState source={source} /></TableCell>
                        <TableCell className="min-w-[190px] align-top text-sm text-aurora-text-secondary">
                          <div className="flex items-center gap-1.5">
                            <Input
                              aria-label={`Refresh interval seconds for ${presentation.label}`}
                              className="h-8 w-28 font-mono text-xs"
                              type="number"
                              min={1}
                              max={31_536_000}
                              step={1}
                              value={intervalDraft}
                              onChange={event => setIntervalDrafts(current => ({ ...current, [source.id]: event.target.value }))}
                            />
                            <Button size="sm" variant="outline" disabled={rowBusy || !intervalChanged} onClick={() => void mutate('interval-' + source.id, () => configureDepotSource(source.id, { intervalSeconds: parsedInterval }))}>Save</Button>
                          </div>
                          <div className="mt-1 text-xs text-aurora-text-muted">seconds · next {formatTimestamp(source.nextAttemptAt)}</div>
                        </TableCell>
                        <TableCell className="max-w-[180px] align-top font-mono text-xs text-aurora-text-secondary">
                          {source.resolvedRevision ?? source.artifactRevision ?? 'Pending'}
                        </TableCell>
                        <TableCell className="max-w-[240px] align-top text-sm text-aurora-text-secondary">
                          <div>{source.lastSuccessAt ? 'Succeeded ' + formatTimestamp(source.lastSuccessAt) : 'No successful refresh yet'}</div>
                          {source.lastError ? <div className="mt-1 line-clamp-3 text-xs text-destructive">{source.lastError}</div> : null}
                          {(source.consecutiveFailures ?? 0) > 0 ? <div className="mt-1 text-xs text-aurora-text-muted">{source.consecutiveFailures} consecutive failures</div> : null}
                        </TableCell>
                        <TableCell className="align-top">
                          <div className="flex justify-end gap-1">
                            <Button size="icon-sm" variant="ghost" disabled={rowBusy} title="Refresh now" onClick={() => void mutate('refresh-' + source.id, () => refreshDepotSource(source.id))}>
                              <RefreshCw className="size-4" />
                            </Button>
                            <Button size="icon-sm" variant="ghost" disabled={rowBusy} title={source.enabled ? 'Pause scheduled refresh' : 'Resume scheduled refresh'} onClick={() => void mutate('configure-' + source.id, () => configureDepotSource(source.id, { enabled: !source.enabled }))}>
                              {source.enabled ? <CirclePause className="size-4" /> : <CirclePlay className="size-4" />}
                            </Button>
                            <Button size="icon-sm" variant="ghost" disabled={rowBusy} title="Delete source" onClick={() => setDeleteCandidate(source)}>
                              <Trash2 className="size-4" />
                            </Button>
                          </div>
                        </TableCell>
                      </TableRow>
                    )
                  })}
                </TableBody>
              </Table>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Add repository source</CardTitle>
            <CardDescription>Start a durable repo ingest. A successful ingest persists the source and schedules refreshes.</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-2"><Label htmlFor="depot-source-url">Repository URL</Label><Input id="depot-source-url" value={url} onChange={event => setUrl(event.target.value)} placeholder="https://github.com/org/repo" /></div>
            <div className="space-y-2"><Label htmlFor="depot-source-namespace">Namespace</Label><Input id="depot-source-namespace" value={namespace} onChange={event => setNamespace(event.target.value)} placeholder="team-skills" /></div>
            <div className="grid gap-4 sm:grid-cols-2">
              <div className="space-y-2"><Label htmlFor="depot-source-ref">Ref</Label><Input id="depot-source-ref" value={ref} onChange={event => setRef(event.target.value)} placeholder="main" /></div>
              <div className="space-y-2"><Label htmlFor="depot-source-subdir">Subdirectory</Label><Input id="depot-source-subdir" value={subdir} onChange={event => setSubdir(event.target.value)} placeholder="skills" /></div>
            </div>
            <div className="space-y-2"><Label htmlFor="depot-source-credential">Credential reference</Label><Input id="depot-source-credential" value={credential} onChange={event => setCredential(event.target.value)} placeholder="github-private" /><p className="text-xs text-aurora-text-muted">A reference to Depot-managed Git credentials. Secrets are never stored in source metadata.</p></div>
            <Button className="w-full" disabled={busy === 'add-repository' || !url.trim() || !namespace.trim()} onClick={() => void addRepository()}>
              {busy === 'add-repository' ? <Loader2 className="size-4 animate-spin" /> : <GitBranch className="size-4" />}
              Start repository ingest
            </Button>
            <div className="rounded-md border border-aurora-border-subtle bg-aurora-control-surface p-3 text-xs leading-5 text-aurora-text-muted">
              Repository creation is first-class here for the Team Depot workflow. Well-known, ARD, marketplace, MCP, MCP Registry, and ACP Registry sources use Depot’s canonical ingest schemas and appear in this inventory after creation.
              {onOpenCatalog ? <Button className="mt-2" size="sm" variant="outline" onClick={onOpenCatalog}>Open all ingest operations</Button> : null}
            </div>
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader className="flex-row items-start justify-between gap-4">
          <div><CardTitle>Recent ingest jobs</CardTitle><CardDescription>Durable Depot job state across every ingest and scheduled source refresh.</CardDescription></div>
          <Button size="sm" variant="outline" disabled={loading} onClick={() => void load()}><RefreshCw className="size-4" />Refresh</Button>
        </CardHeader>
        <CardContent>
          {jobsError ? <div role="status" className="mb-3 rounded-md border border-aurora-warn/40 bg-aurora-warn/10 px-3 py-2 text-sm text-aurora-text-muted"><strong className="text-aurora-warn">Ingest history unavailable.</strong> Source administration remains usable. {jobsError}</div> : null}
          <div className="overflow-x-auto rounded-lg border border-aurora-border-strong">
            <Table>
              <TableHeader><TableRow><TableHead>Job</TableHead><TableHead>Kind</TableHead><TableHead>Status</TableHead><TableHead>Started</TableHead><TableHead>Finished</TableHead><TableHead className="text-right">Actions</TableHead></TableRow></TableHeader>
              <TableBody>
                {jobsError && jobs.length === 0 ? <TableRow><TableCell colSpan={6} className="py-8 text-center text-aurora-text-muted">Recent ingest jobs could not be loaded.</TableCell></TableRow> : jobs.length === 0 ? <TableRow><TableCell colSpan={6} className="py-8 text-center text-aurora-text-muted">No recent ingest jobs.</TableCell></TableRow> : jobs.map(job => {
                  const retryable = ['failed', 'cancelled', 'canceled'].includes(job.status.toLowerCase())
                  const cancellable = ['queued', 'pending', 'running', 'started'].includes(job.status.toLowerCase())
                  return <TableRow key={job.id}>
                    <TableCell className="font-mono text-xs">{job.id}</TableCell>
                    <TableCell>{job.kind ?? 'unknown'}</TableCell>
                    <TableCell><Badge variant="outline" className={job.status.toLowerCase() === 'failed' ? 'border-destructive/40 bg-destructive/10 text-destructive' : undefined}>{job.status}</Badge></TableCell>
                    <TableCell>{formatTimestamp(job.startedAt ?? job.createdAt)}</TableCell>
                    <TableCell>{formatTimestamp(job.finishedAt)}</TableCell>
                    <TableCell><div className="flex justify-end gap-1">
                      {retryable ? <Button size="icon-sm" variant="ghost" title="Retry job" disabled={busy === 'retry-' + job.id} onClick={() => void mutate('retry-' + job.id, () => retryDepotIngestJob(job.id))}><RotateCcw className="size-4" /></Button> : null}
                      {cancellable ? <Button size="sm" variant="outline" disabled={busy === 'cancel-' + job.id} onClick={() => void mutate('cancel-' + job.id, () => cancelDepotIngestJob(job.id))}>Cancel</Button> : null}
                    </div></TableCell>
                  </TableRow>
                })}
              </TableBody>
            </Table>
          </div>
        </CardContent>
      </Card>

      <AlertDialog open={Boolean(deleteCandidate)} onOpenChange={open => { if (!open) setDeleteCandidate(null) }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete persisted source?</AlertDialogTitle>
            <AlertDialogDescription>
              This removes the persisted source and its refresh schedule. Already installed artifacts and skills remain in Depot.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (!deleteCandidate) return
                const source = deleteCandidate
                setDeleteCandidate(null)
                void mutate('delete-' + source.id, () => deleteDepotSource(source.id))
              }}
            >Delete source</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
