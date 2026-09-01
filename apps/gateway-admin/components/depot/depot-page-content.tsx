'use client'

import { useCallback, useEffect, useState } from 'react'
import { useSearchParams } from 'next/navigation'
import { Archive, Box, Loader2, RefreshCw, Search, ShieldCheck } from 'lucide-react'
import { toast } from 'sonner'

import { AppHeader } from '@/components/app-header'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { depotCall, depotStatus, type DepotArtifact, type DepotStatus } from '@/lib/api/depot-client'

type LoadState = { loading: boolean; error?: string; status?: DepotStatus; artifacts: DepotArtifact[]; cursor?: string; total?: number }

export function DepotPageContent() {
  const searchParams = useSearchParams()
  const selectedId = searchParams.get('artifact')?.trim()
  const [query, setQuery] = useState('')
  const [activeQuery, setActiveQuery] = useState('')
  const [state, setState] = useState<LoadState>({ loading: true, artifacts: [] })
  const [detail, setDetail] = useState<DepotArtifact | null>(null)

  const load = useCallback(async (searchQuery: string, cursor?: string, signal?: AbortSignal) => {
    setState((current) => ({ ...current, loading: true, error: undefined }))
    try {
      const [status, listing] = await Promise.all([
        depotStatus(signal),
        depotCall<{ result?: { artifacts?: DepotArtifact[]; nextCursor?: string; total?: number } }>('depot.artifacts.list', { limit: 50, ...(searchQuery ? { query: searchQuery } : {}), ...(cursor ? { cursor } : {}) }, signal),
      ])
      setState((current) => ({ loading: false, status, artifacts: cursor ? [...current.artifacts, ...(listing.result?.artifacts ?? [])] : (listing.result?.artifacts ?? []), cursor: listing.result?.nextCursor, total: listing.result?.total }))
    } catch (error) {
      if (signal?.aborted) return
      setState((current) => ({ ...current, loading: false, error: error instanceof Error ? error.message : String(error) }))
    }
  }, [])

  useEffect(() => {
    const controller = new AbortController()
    const timer = window.setTimeout(() => {
      const nextQuery = query.trim()
      setActiveQuery(nextQuery)
      void load(nextQuery, undefined, controller.signal)
    }, query ? 300 : 0)

    return () => { window.clearTimeout(timer); controller.abort() }
  }, [load, query])
  const loadDetail = useCallback(async (artifactId: string, signal?: AbortSignal) => {
    const response = await depotCall<{ result?: { artifact?: DepotArtifact } }>('depot.artifacts.get', { artifactId }, signal)
    setDetail(response.result?.artifact ?? null)
  }, [])
  useEffect(() => {
    if (!selectedId) { setDetail(null); return }
    const controller = new AbortController()
    void loadDetail(selectedId, controller.signal)
      .catch((error) => { if (!controller.signal.aborted) toast.error(error instanceof Error ? error.message : String(error)) })
    return () => controller.abort()
  }, [loadDetail, selectedId])

  return <>
    <AppHeader breadcrumbs={[{ label: 'Depot' }, ...(selectedId ? [{ label: selectedId }] : [])]} />
    <div className={`${AURORA_PAGE_SHELL} flex-1`}><div className={AURORA_PAGE_FRAME}>
      <ConsoleHero eyebrow="Unified control plane" title="Depot Bazaar" pulse={{ color: state.status?.enabled ? 'var(--aurora-success)' : 'var(--aurora-warn)', label: state.status?.enabled ? 'connected through Labby' : 'disabled' }} actions={<Button variant="outline" size="sm" onClick={() => void load(activeQuery)} disabled={state.loading}>{state.loading ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />}Refresh</Button>} stats={[
        { label: activeQuery ? 'Search results' : 'Artifacts loaded', value: state.total === undefined ? state.artifacts.length : `${state.artifacts.length} / ${state.total}`, icon: <Archive size={12}/> },
        { label: 'Authority', value: 'Read only', icon: <ShieldCheck size={12}/> },
        { label: 'Page limit', value: 50, icon: <Box size={12}/> },
      ]}/>
      {state.error ? <DashboardPanel title="Depot unavailable"><p role="alert" className="text-sm text-aurora-error">{state.error}. Labby-only routes remain available.</p></DashboardPanel> : null}
      <DashboardPanel title="Browse immutable artifacts" icon={<Search className="size-4"/>} action={<div className="relative"><Search aria-hidden="true" className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-aurora-text-muted"/><Input aria-label="Search all Bazaar artifacts" className="h-8 w-72 pl-8" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search the full Bazaar"/></div>}>
        <p className="pb-3 text-xs text-aurora-text-muted">Depot artifacts are published catalog entries. Connected MCP gateways remain available under Gateways and are not automatically published here.</p>
        <div className="divide-y divide-aurora-border-subtle">
          {state.artifacts.map((artifact) => { const id = artifact.id ?? 'unknown'; return <a key={id} href={`/depot?artifact=${encodeURIComponent(id)}`} className="flex min-h-14 items-center justify-between gap-4 rounded px-2 py-2 hover:bg-aurora-surface-muted focus-visible:outline-none focus-visible:ring-2"><div><div className="font-medium">{artifact.name ?? id}</div><div className="text-xs text-aurora-text-muted">{artifact.kind} · {artifact.namespace} · {id}</div>{artifact.description ? <p className="mt-1 line-clamp-2 text-xs text-aurora-text-muted">{artifact.description}</p> : null}</div><Badge variant="outline">{artifact.publication?.visibility ?? 'private'}</Badge></a> })}
          {state.loading && state.artifacts.length === 0 ? <p className="flex items-center justify-center gap-2 py-8 text-sm text-aurora-text-muted"><Loader2 className="size-4 animate-spin"/>Searching Bazaar…</p> : null}
          {!state.loading && state.artifacts.length === 0 ? <p className="py-8 text-center text-sm text-aurora-text-muted">{activeQuery ? `No Bazaar artifacts match “${activeQuery}”.` : 'No artifacts are published in Bazaar.'}</p> : null}
        </div>
        {state.cursor ? <Button variant="outline" onClick={() => void load(activeQuery, state.cursor)} disabled={state.loading}>{state.loading ? <Loader2 className="size-4 animate-spin"/> : null}Load more</Button> : null}
      </DashboardPanel>
      {detail && selectedId ? <DashboardPanel title="Artifact detail" icon={<Box className="size-4"/>}>
        <h2 className="text-lg font-semibold">{detail.name ?? selectedId}</h2><p className="text-sm text-aurora-text-muted">{detail.description ?? 'No description supplied.'}</p><code className="break-all text-xs">{detail.currentRevisionId}</code>
        <p className="text-xs text-aurora-text-muted">Mutation and exact-import controls remain unavailable until Labby negotiates a compatible delegated authority epoch with Depot.</p>
      </DashboardPanel> : null}
    </div></div>
  </>
}
