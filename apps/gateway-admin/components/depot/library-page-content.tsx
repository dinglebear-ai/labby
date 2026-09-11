'use client'

import { useCallback, useEffect, useId, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import { useSearchParams } from 'next/navigation'
import { Archive, Box, Check, ChevronDown, ChevronRight, Copy, Download, ExternalLink, FileText, Filter, Globe, Grid2X2, Link2, List, Loader2, LockKeyhole, RefreshCw, Search, ShieldCheck, Table2, Users, X } from 'lucide-react'
import { toast } from 'sonner'

import { AppHeader } from '@/components/app-header'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogFooter } from '@/components/ui/dialog'
import { depotPublishCapability, depotStatus, type DepotArtifact, type DepotPublishCapability, type DepotStatus } from '@/lib/api/depot-client'
import { controlPlaneAction } from '@/lib/api/artifact-control-client'
import { LibraryTabs } from '@/components/depot/depot-workspace-pages'
import { getBrowserSessionEpoch, subscribeToBrowserSession } from '@/lib/auth/session-store'
import { artifactDescription, artifactExportFilename, artifactId, artifactKind, artifactLabel, collectArtifactKinds, collectArtifactTags, filterLibraryArtifacts, sortLibraryArtifacts, serializeArtifact } from './library-model'
import { ARTIFACT_TYPES, ArtifactTypeMark, artifactTypeDefinition } from './artifact-type'
import { updateLibraryUrl as updateUrl } from './library-url'

type LibraryState = {
  artifacts: DepotArtifact[]
  cursor?: string
  error?: string
  loading: boolean
  status?: DepotStatus
  publishing?: DepotPublishCapability
  total?: number
}

const PAGE_SIZE = 50
type ViewMode = 'table' | 'list' | 'cards'

export function libraryFilterKinds(artifacts: DepotArtifact[]) {
  return [...new Set([...ARTIFACT_TYPES, ...collectArtifactKinds(artifacts)])]
}

export function LibraryFilterRail({ artifacts, kind, onKind, tag, onTag }: { artifacts: DepotArtifact[]; kind: string; onKind: (kind: string) => void; tag?: string; onTag?: (tag: string | undefined) => void }) {
  const [expanded, setExpanded] = useState(false)
  const filtersId = useId()
  return <aside aria-label="Library filters" data-lbrail="1" className="min-w-0 self-start lg:sticky lg:top-3">
    <button type="button" aria-expanded={expanded} aria-controls={filtersId} onClick={() => setExpanded(value => !value)} className="flex min-h-11 w-full items-center gap-2 rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-strong px-3 text-left text-xs text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary min-[901px]:hidden">
      <Filter aria-hidden="true" className="size-3.5 shrink-0"/><span className="min-w-0 flex-1 truncate">Filters · {kind === 'all' ? 'All artifacts' : artifactTypeDefinition(kind).label}{tag ? ` · ${tag}` : ''}</span><ChevronDown aria-hidden="true" className={`size-3.5 shrink-0 transition-transform ${expanded ? 'rotate-180' : ''}`}/>
    </button>
    <div id={filtersId} className={`${expanded ? 'block' : 'hidden'} space-y-3 max-[900px]:mt-3 min-[901px]:block`}>
    <div className="rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-strong p-2 shadow-[var(--aurora-shadow-medium)]">
      <p className="px-2 pb-2 pt-1 text-[10px] font-semibold text-aurora-text-muted">Artifact types · loaded results</p>
      {['all', ...libraryFilterKinds(artifacts)].map(value => {
        const definition = artifactTypeDefinition(value)
        const Icon = value === 'all' ? Box : definition.icon
        const count = value === 'all' ? artifacts.length : artifacts.filter(artifact => artifactKind(artifact) === value).length
        return <button key={value} type="button" aria-pressed={kind === value} onClick={() => onKind(value)} className="flex min-h-[30px] w-full items-center gap-2 rounded-lg border border-transparent px-2 text-left text-[12.5px] font-semibold text-aurora-text-muted hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary aria-pressed:border-aurora-border-strong aria-pressed:bg-aurora-selected-bg aria-pressed:text-aurora-text-primary">
          <Icon aria-hidden="true" className="size-3.5 shrink-0" /><span className="min-w-0 flex-1 truncate">{value === 'all' ? 'All artifacts' : definition.label}</span><span className="text-[10.5px] tabular-nums opacity-75">{count}</span>
        </button>
      })}
    </div>
    <section aria-label="Tags" className="overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-strong">
      <h2 className="border-b border-aurora-border-subtle bg-aurora-control-surface px-3 py-2 text-[10px] font-semibold text-aurora-text-muted">Tags · loaded results</h2>
      <div className="flex flex-wrap gap-1.5 p-3">
        {collectArtifactTags(artifacts).map(({ tag: value, count }) => <Button data-visible-label="1" key={value} variant="outline" size="sm" aria-pressed={tag === value} onClick={() => onTag?.(tag === value ? undefined : value)} className="h-[23px] max-w-full gap-1.5 rounded-md px-2 text-[10.5px] aria-pressed:border-aurora-accent-primary aria-pressed:bg-aurora-selected-bg"><span className="truncate">{value}</span><span className="tabular-nums text-aurora-text-muted">{count}</span></Button>)}
        {collectArtifactTags(artifacts).length === 0 ? <p className="text-xs leading-5 text-aurora-text-muted">No tags supplied in loaded results.</p> : null}
      </div>
    </section>
    </div>
  </aside>
}

export function LibraryVisibility({ visibility }: { visibility?: string }) {
  const definitions: Record<string, { label: string; icon: typeof Globe; color: string }> = {
    public: { label: 'Public', icon: Globe, color: 'var(--aurora-success)' },
    team: { label: 'Team', icon: Users, color: 'var(--aurora-accent-strong)' },
    private: { label: 'Private', icon: LockKeyhole, color: 'var(--aurora-text-muted)' },
  }
  const key = visibility?.toLowerCase() ?? ''
  const known = Object.hasOwn(definitions, key) ? definitions[key] : undefined
  if (!known) return <span className="text-aurora-text-muted">{visibility || 'Unknown'}</span>
  const Icon = known.icon
  return <span className="inline-flex items-center gap-[5px]" style={{ color: known.color }}><Icon aria-hidden="true" className="size-3" />{known.label}</span>
}

export function LibraryArtifactTable({ artifacts, onInspect }: { artifacts: DepotArtifact[]; onInspect: (id: string) => void }) {
  return <div className="aurora-scrollbar max-h-[56vh] overflow-auto">
    <table aria-label="Library artifacts" className="w-full min-w-[560px] table-fixed text-left">
      <colgroup><col className="w-[106px]"/><col/><col className="w-[132px]"/><col className="w-[92px]"/><col className="w-[42px]"/></colgroup>
      <thead className="sticky top-0 z-10 bg-aurora-panel-strong"><tr className="border-b border-aurora-border-subtle text-[9.5px] font-bold uppercase tracking-[.1em] text-aurora-text-muted">
        {['Kind', 'Artifact', 'Tags', 'Visibility'].map(label => <th key={label} scope="col" className="px-2 py-2 first:pl-4">{label}</th>)}
        <th scope="col" className="px-2 py-2"><span className="sr-only">Open</span></th>
      </tr></thead>
      <tbody>{artifacts.map(artifact => {
        const id = artifactId(artifact)
        return <tr key={id} onClick={() => onInspect(id)} className="group cursor-pointer border-b border-aurora-border-subtle/70 last:border-b-0 hover:bg-aurora-surface-muted focus-within:bg-aurora-surface-muted">
          <td className="py-[9px] pl-4 pr-2"><ArtifactTypeMark artifact={artifact} compact/></td>
          <td className="px-2 py-[9px]"><button type="button" onClick={event => { event.stopPropagation(); onInspect(id) }} aria-label={`Inspect ${artifactLabel(artifact)}`} className="block w-full min-w-0 rounded text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">
            <span title={artifact.namespace ?? artifact.descriptor?.namespace} className="block truncate text-[12.5px] font-semibold leading-[18px] text-aurora-text-primary">{artifactLabel(artifact)}</span>
            <span className="block truncate text-[10.5px] leading-4 text-aurora-text-muted">{artifactDescription(artifact)}</span>
          </button></td>
          <td className="px-2 py-[9px] text-[10.5px] text-aurora-text-muted"><div className="flex gap-1 overflow-hidden" title={artifact.descriptor?.tags?.join(', ')}>{artifact.descriptor?.tags?.length ? artifact.descriptor.tags.map(tag => <span key={tag} className="h-[17px] max-w-[116px] shrink-0 truncate rounded border border-[color-mix(in_srgb,var(--aurora-accent-primary)_22%,transparent)] bg-[color-mix(in_srgb,var(--aurora-accent-primary)_8%,transparent)] px-[7px] text-[9.5px] font-[650] leading-[15px] text-aurora-accent-strong">{tag}</span>) : '—'}</div></td>
          <td className="truncate px-2 py-[9px] text-[10.5px] font-semibold"><LibraryVisibility visibility={artifact.publication?.visibility}/></td>
          <td className="py-[9px] pl-2 pr-4"><ChevronRight aria-hidden="true" className="size-3.5 text-aurora-text-muted"/></td>
        </tr>
      })}</tbody>
    </table>
  </div>
}

export function LibrarySortMenu({ sort, onSort }: { sort: 'catalog' | 'name' | 'kind'; onSort: (sort: 'catalog' | 'name' | 'kind') => void }) {
  const choices = [{ value: 'catalog', label: 'Catalog order' }, { value: 'name', label: 'Name' }, { value: 'kind', label: 'Kind' }] as const
  return <Popover><PopoverTrigger asChild><Button data-visible-label="1" variant="outline" size="sm" aria-label="Sort loaded library results" className="h-[26px] rounded-full px-[11px] text-[11px]">{choices.find(choice => choice.value === sort)?.label}<ChevronDown className="size-3"/></Button></PopoverTrigger><PopoverContent align="end" className="w-48 p-2"><p className="px-2 py-1 text-[10.5px] text-aurora-text-muted">Sort loaded results</p>{choices.map(choice => <Button data-visible-label="1" key={choice.value} variant="ghost" size="sm" aria-pressed={sort === choice.value} onClick={() => onSort(choice.value)} className="w-full justify-between text-xs">{choice.label}{sort === choice.value ? <Check className="size-3.5"/> : null}</Button>)}</PopoverContent></Popover>
}

export function LibraryPageContent() {
  const sessionEpoch = useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionEpoch, () => 0)
  // Session changes invalidate both retained data and every in-flight read.
  return <SessionLibraryPage key={sessionEpoch} />
}

function SessionLibraryPage() {
  const searchParams = useSearchParams()
  const selectedId = searchParams.get('artifact')?.trim() ?? ''
  const initialQuery = searchParams.get('q')?.trim() ?? ''
  const [query, setQuery] = useState(initialQuery)
  const [activeQuery, setActiveQuery] = useState(initialQuery)
  const loadedQuery = useRef(initialQuery)
  const [tagSelection, setTagSelection] = useState<{ query: string; tag?: string }>({ query: initialQuery })
  const tag = tagSelection.query === query ? tagSelection.tag : undefined
  const [sort, setSort] = useState<'catalog' | 'name' | 'kind'>('catalog')
  const [kind, setKind] = useState(searchParams.get('kind')?.trim().toLocaleLowerCase() || 'all')
  const [state, setState] = useState<LibraryState>({ artifacts: [], loading: true })
  const [detailResult, setDetail] = useState<{ selectedId: string; artifact: DepotArtifact } | null>(null)
  const detail = detailResult?.selectedId === selectedId ? detailResult.artifact : null
  const [detailLoading, setDetailLoading] = useState(false)
  const [copied, setCopied] = useState<string>()
  const [view, setViewState] = useState<ViewMode>('table')
  const viewSelectedByUser = useRef(false)
  const listController = useRef<AbortController | null>(null)
  const setView = useCallback((next: ViewMode) => {
    viewSelectedByUser.current = true
    setViewState(next)
  }, [])

  useEffect(() => {
    const media = window.matchMedia('(max-width: 640px)')
    const applyResponsiveDefault = () => {
      if (!viewSelectedByUser.current) setViewState(media.matches ? 'cards' : 'table')
    }
    applyResponsiveDefault()
    media.addEventListener('change', applyResponsiveDefault)
    return () => media.removeEventListener('change', applyResponsiveDefault)
  }, [])

  const load = useCallback(async (search: string, cursor?: string) => {
    listController.current?.abort()
    const controller = new AbortController()
    listController.current = controller
    const signal = controller.signal
    const loadingSessionEpoch = getBrowserSessionEpoch()
    const isCurrent = () => !signal.aborted && loadingSessionEpoch === getBrowserSessionEpoch()
    setState((current) => cursor
      ? { ...current, loading: true, error: undefined, publishing: undefined }
      : { artifacts: [], loading: true })
    try {
      // Read the server's live status before issuing caller-bound library reads.
      const status = await depotStatus(signal)
      if (!isCurrent()) return
      const [response, publishing] = await Promise.all([
        controlPlaneAction<{ artifacts?: DepotArtifact[]; nextCursor?: string; total?: number }>(
          'artifacts',
          'artifacts.list_remote',
          { limit: PAGE_SIZE, ...(search ? { query: search } : {}), ...(cursor ? { cursor } : {}) },
          signal,
        ),
        depotPublishCapability(signal).catch(() => undefined),
      ])
      if (!isCurrent()) return
      setState((current) => ({
        artifacts: cursor ? [...current.artifacts, ...(response.artifacts ?? [])] : (response.artifacts ?? []),
        cursor: response.nextCursor,
        loading: false,
        status,
        publishing,
        total: response.total,
      }))
    } catch (error) {
      if (isCurrent()) setState((current) => ({ ...current, error: error instanceof Error ? error.message : String(error), loading: false }))
    }
  }, [])

  useEffect(() => {
    const timer = window.setTimeout(() => {
      const next = query.trim()
      setActiveQuery(next)
      setTagSelection({ query })
      // A cold load must preserve an artifact deep link. Only a changed search
      // invalidates the selected result; session remounts still discard data.
      updateUrl({ ...(next !== loadedQuery.current ? { artifact: null } : {}), q: next })
      loadedQuery.current = next
      void load(next)
    }, query ? 300 : 0)
    return () => { window.clearTimeout(timer); listController.current?.abort() }
  }, [load, query])

  useEffect(() => {
    setDetail(null)
    setDetailLoading(false)
    if (!selectedId) return
    const controller = new AbortController()
    const loadingSessionEpoch = getBrowserSessionEpoch()
    const isCurrent = () => !controller.signal.aborted && loadingSessionEpoch === getBrowserSessionEpoch()
    setDetailLoading(true)
    void depotStatus(controller.signal)
      .then(() => isCurrent() ? controlPlaneAction<{ artifact?: DepotArtifact }>('artifacts', 'artifacts.get_remote', { id: selectedId }, controller.signal) : undefined)
      .then((response) => { if (isCurrent()) setDetail(response?.artifact ? { selectedId, artifact: response.artifact } : null) })
      .catch((error) => { if (isCurrent()) toast.error(error instanceof Error ? error.message : String(error)) })
      .finally(() => { if (isCurrent()) setDetailLoading(false) })
    return () => controller.abort()
  }, [selectedId])

  const kinds = useMemo(() => collectArtifactKinds(state.artifacts), [state.artifacts])
  const visible = useMemo(() => sortLibraryArtifacts(filterLibraryArtifacts(state.artifacts, kind, tag), sort), [kind, tag, sort, state.artifacts])
  const copy = useCallback(async (label: string, value: string) => {
    await navigator.clipboard.writeText(value)
    setCopied(label)
    toast.success(`${label} copied`)
    window.setTimeout(() => setCopied((current) => current === label ? undefined : current), 1_500)
  }, [])
  const exportArtifact = useCallback((artifact: DepotArtifact) => {
    const url = URL.createObjectURL(new Blob([serializeArtifact(artifact)], { type: 'application/json' }))
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = artifactExportFilename(artifact)
    anchor.click()
    URL.revokeObjectURL(url)
    toast.success('Artifact metadata exported')
  }, [])
  const shareArtifact = useCallback(async () => {
    await copy('Share link', window.location.href)
  }, [copy])

  return <>
    <AppHeader breadcrumbs={[{ label: 'Depot' }, { label: 'Library' }]} />
    <div className={`${AURORA_PAGE_SHELL} min-w-0 flex-1`}><div className={`${AURORA_PAGE_FRAME} gap-3.5`}>
      <ConsoleHero eyebrow="Depot · Library" title="Library" footer={<LibraryTabs active="artifacts" attached counts={state.error || state.total === undefined ? {} : { artifacts: state.total }} />} pulse={{ color: state.status?.enabled ? 'var(--aurora-success)' : 'var(--aurora-warn)', label: state.status?.enabled ? 'live catalog' : 'Depot unavailable' }} actions={<div className="flex flex-wrap gap-2"><Button variant="outline" size="sm" asChild><a href="/depot"><Search className="size-4"/>Discover</a></Button><Button variant="outline" size="sm" disabled={state.loading} onClick={() => void load(activeQuery)}>{state.loading ? <Loader2 className="size-4 animate-spin"/> : <RefreshCw className="size-4"/>}Refresh</Button></div>} stats={[
        { label: activeQuery ? 'Matches' : 'Published artifacts', value: state.total ?? '—', icon: <Archive size={12}/> },
        { label: 'Loaded', value: state.artifacts.length, icon: <Box size={12}/> },
        { label: 'Kinds loaded', value: kinds.length, icon: <FileText size={12}/> },
        { label: 'Your access', value: state.publishing?.available ? 'Read + publish' : state.publishing?.reason === 'owner_link_approval_pending' ? 'Confirm owner link' : state.publishing ? 'Read only' : 'Unknown', icon: <ShieldCheck size={12}/> },
      ]}/>
      {state.error ? <DashboardPanel title="Depot unavailable"><p role="alert" className="text-sm text-aurora-error">{state.error}. Refresh after Depot is connected.</p></DashboardPanel> : null}
      <div data-lbgrid="1" className="grid min-w-0 items-start gap-3.5 min-[901px]:grid-cols-[214px_minmax(0,1fr)]">
      <LibraryFilterRail artifacts={state.artifacts} kind={kind} onKind={next => { setKind(next); updateUrl({ kind: next }) }} tag={tag} onTag={next => setTagSelection({ query, tag: next })} />
      <div className="min-w-0">
      <section aria-label="Artifact collection" data-library-collection="1" className="overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-strong shadow-[var(--aurora-shadow-medium)]"><div data-library-toolbar="1" className="flex flex-wrap items-center gap-[9px] border-b border-aurora-border-subtle bg-aurora-control-surface px-[13px] py-[9px]"><div className="relative min-w-24 max-w-[320px] flex-1"><Search className="absolute left-[11px] top-1/2 size-[13px] -translate-y-1/2 text-aurora-text-muted"/><Input aria-label="Search library" className="h-[30px] w-full rounded-[9px] pl-8 pr-8 text-[12.5px]" placeholder="Search the full Depot catalog…" value={query} onChange={(event) => setQuery(event.target.value)}/>{query ? <button type="button" aria-label="Clear library search" className="absolute right-2 top-1/2 -translate-y-1/2 p-1 text-aurora-text-muted" onClick={() => setQuery('')}><X className="size-4"/></button> : null}</div><span className="text-[11px] font-semibold tabular-nums text-aurora-text-muted">{visible.length} shown</span><div className="flex-1"/><LibrarySortMenu sort={sort} onSort={setSort}/><Popover><PopoverTrigger asChild><Button variant="outline" size="sm" aria-label="Filter library by artifact type" className={`h-[26px] rounded-full px-[11px] text-[11px] ${kind !== 'all' ? 'border-aurora-accent-primary text-aurora-text-primary' : ''}`}><Filter className="size-3.5"/>{kind === 'all' ? 'Filters' : artifactTypeDefinition(kind).label}<ChevronDown className="size-3.5"/></Button></PopoverTrigger><PopoverContent align="end" className="w-64 p-2"><div className="px-2 pb-2 pt-1"><p className="text-xs font-semibold text-aurora-text-primary">Artifact type</p><p className="mt-0.5 text-[11px] text-aurora-text-muted">Show one catalog family at a time.</p></div><button type="button" onClick={() => { setKind('all'); updateUrl({ kind: 'all' }) }} aria-pressed={kind === 'all'} className="flex w-full items-center gap-2 rounded-aurora-1 px-2 py-2 text-left text-xs text-aurora-text-muted hover:bg-aurora-hover-bg aria-pressed:bg-aurora-selected-bg aria-pressed:text-aurora-text-primary"><span className="grid size-7 place-items-center rounded-aurora-1 border border-aurora-border-subtle"><Box className="size-3.5"/></span><span className="flex-1 font-semibold">All artifacts</span>{kind === 'all' ? <Check className="size-4 text-aurora-accent-primary"/> : null}</button>{libraryFilterKinds(state.artifacts).map((item) => { const definition = artifactTypeDefinition(item); const Icon = definition.icon; return <button key={item} type="button" onClick={() => { setKind(item); updateUrl({ kind: item }) }} aria-pressed={kind === item} className="flex w-full items-center gap-2 rounded-aurora-1 px-2 py-2 text-left text-xs text-aurora-text-muted hover:bg-aurora-hover-bg aria-pressed:bg-aurora-selected-bg aria-pressed:text-aurora-text-primary"><span className="grid size-7 place-items-center rounded-aurora-1 border" style={{ color: definition.color, borderColor: `color-mix(in srgb, ${definition.color} 38%, transparent)` }}><Icon className="size-3.5"/></span><span className="flex-1 font-semibold">{definition.label}</span>{kind === item ? <Check className="size-4 text-aurora-accent-primary"/> : null}</button> })}</PopoverContent></Popover><div className="hidden rounded-aurora-1 border border-aurora-border-subtle bg-aurora-control-surface p-0.5 sm:flex">{([[Table2,'Table','table'],[List,'List','list'],[Grid2X2,'Cards','cards']] as const).map(([Icon,label,mode]) => <button key={mode} type="button" aria-label={`${label} view`} title={`${label} view`} aria-pressed={view === mode} onClick={() => setView(mode)} className="rounded p-1.5 text-aurora-text-muted transition-colors hover:text-aurora-text-primary aria-pressed:bg-aurora-selected-bg aria-pressed:text-aurora-accent-primary"><Icon className="size-3.5"/></button>)}</div></div>
        <div className="flex flex-wrap items-center gap-2 border-b border-aurora-border-subtle px-4 py-2">
          <span className="text-[10.5px] font-semibold text-aurora-text-primary">{kind === 'all' ? 'All artifact types' : artifactTypeDefinition(kind).label}</span>
          {kind !== 'all' ? <button type="button" onClick={() => { setKind('all'); updateUrl({ kind: 'all' }) }} className="inline-flex items-center gap-1 rounded-full border border-aurora-border-subtle px-2 py-1 text-[11px] text-aurora-text-muted hover:text-aurora-text-primary">Clear filter<X className="size-3"/></button> : null}
          <span className="ml-auto text-[10.5px] text-aurora-text-muted">{state.artifacts.length} loaded · {state.total ?? 0} catalog total</span>
        </div>
        {view !== 'table' ? <div className={view === 'cards' ? 'grid gap-3 p-3 sm:grid-cols-2 xl:grid-cols-3' : 'divide-y divide-aurora-border-subtle'}>{visible.map((artifact) => { const id = artifactId(artifact); return <button key={id} type="button" onClick={() => updateUrl({ artifact: id })} className={view === 'cards' ? 'group rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-low p-4 text-left transition-[transform,border-color] hover:-translate-y-0.5 hover:border-aurora-border-strong' : 'group flex w-full items-start gap-3 px-3 py-3 text-left transition-colors hover:bg-aurora-surface-muted'}><ArtifactTypeMark artifact={artifact} compact/><span className="min-w-0 flex-1"><span className="block truncate font-semibold text-aurora-text-primary">{artifactLabel(artifact)}</span><span className="mt-1 line-clamp-2 block text-xs leading-5 text-aurora-text-muted">{artifactDescription(artifact)}</span><span className="mt-2 block truncate text-[11px] text-aurora-text-muted">{artifact.namespace ?? artifact.descriptor?.namespace ?? 'Unknown namespace'}</span></span><ChevronRight className="mt-1 size-4 shrink-0 text-aurora-text-muted group-hover:text-aurora-accent-primary"/></button> })}</div> : <LibraryArtifactTable artifacts={visible} onInspect={id => updateUrl({ artifact: id })}/>}
        {state.loading && state.artifacts.length === 0 ? <p className="flex items-center justify-center gap-2 py-10 text-sm text-aurora-text-muted"><Loader2 className="size-4 animate-spin"/>Loading the Depot catalog…</p> : null}
        {!state.loading && visible.length === 0 ? <p className="py-10 text-center text-sm text-aurora-text-muted">No loaded artifacts match this view.</p> : null}
        {state.cursor ? <div className="border-t border-aurora-border-subtle p-3 text-center"><Button variant="outline" disabled={state.loading} onClick={() => void load(activeQuery, state.cursor)}>{state.loading ? <Loader2 className="size-4 animate-spin"/> : null}Load 50 more</Button></div> : null}
      </section>
      </div>
      </div>
    </div></div>
    <Dialog open={Boolean(selectedId)} onOpenChange={(open) => { if (!open) updateUrl({ artifact: null }) }}><DialogContent className="flex max-h-[86vh] w-[96vw] max-w-[720px] flex-col gap-0 overflow-hidden rounded-aurora-2 border-aurora-border-subtle bg-gradient-to-b from-aurora-panel-strong-top to-aurora-panel-strong p-0 shadow-[var(--aurora-shadow-strong)] sm:max-w-[720px]">
      <DialogHeader className="shrink-0 border-b border-aurora-border-subtle px-4 py-[15px] pr-14 text-left">
        <div className="flex min-w-0 items-start gap-[11px]">
          {detail ? <span aria-hidden="true" style={artifactTypeDefinition(artifactKind(detail)).iconStyle} className="grid size-[34px] shrink-0 place-items-center rounded-[10px] border">{(() => { const Icon = artifactTypeDefinition(artifactKind(detail)).icon; return <Icon className="size-[18px]"/> })()}</span> : null}
          <div className="min-w-0"><DialogTitle className="truncate font-display text-[18px] font-extrabold tracking-[-.01em]">{detail ? artifactLabel(detail) : 'Artifact details'}</DialogTitle>
          <DialogDescription className="sr-only">{detail ? `${detail.namespace ?? detail.descriptor?.namespace ?? 'unknown'} · ${artifactKind(detail)}` : selectedId}</DialogDescription>
          {detail?.descriptor?.tags?.length ? <ul aria-label="Artifact tags" className="mt-1 flex flex-wrap gap-[5px]">{detail.descriptor.tags.map((tag, index) => <li key={index} className="max-w-full break-all rounded-[5px] border border-aurora-accent-primary/20 bg-aurora-accent-primary/10 px-[9px] text-[10.5px] font-semibold leading-5 text-aurora-accent-strong">{tag}</li>)}</ul> : null}</div>
        </div>
      </DialogHeader>
      {detailLoading ? <p className="flex items-center gap-2 p-5 text-sm text-aurora-text-muted"><Loader2 className="size-4 animate-spin"/>Loading artifact…</p> : detail ? <div className="aurora-scrollbar min-h-0 min-w-0 flex-1 space-y-3.5 overflow-y-auto px-4 pb-5 pt-[15px]"><p className="break-words text-[13px] leading-[1.6] text-aurora-text-muted">{artifactDescription(detail)}</p><div className="flex flex-wrap gap-2"><Badge variant="outline" className="capitalize">{artifactKind(detail)}</Badge><Badge variant="outline">{detail.publication?.state ?? 'unknown state'}</Badge><Badge variant="outline">{detail.publication?.visibility ?? 'unknown visibility'}</Badge></div><details key={selectedId} className="rounded-aurora-2 border border-aurora-border-subtle"><summary className="cursor-pointer p-4 text-[10px] font-bold uppercase tracking-wider text-aurora-text-muted">Source and revision</summary><div className="space-y-3 p-4"><dl className="grid grid-cols-2 gap-px overflow-hidden rounded-aurora-1 border border-aurora-border-subtle bg-aurora-border-subtle">{([
        ['Distribution', detail.publication?.distribution], ['License review', detail.license?.reviewState], ['Redistribution', detail.license?.redistribution], ['Revisions', detail.revisionCount?.toString()],
      ] satisfies Array<[string, string | undefined]>).map(([label, value]) => <div key={label} className="bg-aurora-panel-low p-3"><dt className="text-[10px] font-bold uppercase tracking-wider text-aurora-text-muted">{label}</dt><dd className="mt-1 text-sm text-aurora-text-primary">{value || 'Not supplied'}</dd></div>)}</dl>{([
        ['Artifact ID', artifactId(detail)], ['Revision ID', detail.currentRevisionId ?? detail.currentRevision?.id], ['Content digest', detail.contentDigest ?? detail.currentRevision?.contentDigest],
      ] satisfies Array<[string, string | undefined]>).map(([label, value]) => value ? <div key={label} className="flex items-center gap-2 rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low p-3"><div className="min-w-0 flex-1"><div className="text-[10px] font-bold uppercase tracking-wider text-aurora-text-muted">{label}</div><code className="block truncate pt-1 text-xs text-aurora-text-primary" title={value}>{value}</code></div><Button variant="ghost" size="icon-sm" aria-label={`Copy ${label}`} onClick={() => void copy(label, value)}>{copied === label ? <Check className="size-4 text-aurora-success"/> : <Copy className="size-4"/>}</Button></div> : null)}{detail.currentRevision?.components?.length ? <div><h3 className="mb-2 text-sm font-semibold text-aurora-text-primary">Components</h3><div className="divide-y divide-aurora-border-subtle rounded-aurora-1 border border-aurora-border-subtle">{detail.currentRevision.components.map((component, index) => <div key={component.id ?? index} className="flex justify-between gap-3 p-3 text-xs"><code className="truncate">{component.path ?? component.id}</code><span className="shrink-0 text-aurora-text-muted">{component.mediaType ?? component.kind ?? 'file'}</span></div>)}</div></div> : null}</div></details></div> : <p className="p-5 text-sm text-aurora-text-muted">Artifact details are unavailable.</p>}
    {detail && !detailLoading ? <DialogFooter className="shrink-0 flex-row flex-wrap gap-1.5 border-t border-aurora-border-subtle bg-aurora-control-surface px-4 pb-[11px] pt-[9px] [&_button]:size-8 [&_button]:rounded-[9px] [&_button]:p-0 [&_a]:size-8 [&_a]:p-0">
      <Button size="sm" title="Copy link" onClick={() => void shareArtifact()}><Link2 className="size-4"/><span className="sr-only">{copied === 'Share link' ? 'Link copied' : 'Copy link'}</span></Button>
      <Button size="sm" variant="outline" aria-label="Export JSON" title="Export JSON" onClick={() => exportArtifact(detail)}><Download className="size-4"/><span className="sr-only">Export JSON</span></Button>
      <Button size="sm" variant="outline" asChild><a title="Open in Discover" href={`/depot?artifact=${encodeURIComponent(artifactId(detail))}`}><ExternalLink className="size-4"/><span className="sr-only">Open in Discover</span></a></Button>
    </DialogFooter> : null}</DialogContent></Dialog>
  </>
}
