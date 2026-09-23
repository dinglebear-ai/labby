'use client'

import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import { useSearchParams } from 'next/navigation'
import { Box, Check, ChevronRight, Download, Filter, Globe, GitFork, Loader2, LockKeyhole, Pencil, RefreshCw, Search, Send, Users, X } from 'lucide-react'
import { toast } from 'sonner'

import { AppHeader } from '@/components/app-header'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { depotCall, type DepotArtifact } from '@/lib/api/depot-client'
import { mockDepotMetricLabels, mockDepotSpecLabels } from '@/lib/api/depot-mock-data'
import { localLibraryFederatedArtifact } from './local-library-model'
import { DiscoverArtifactInspection } from './discover-artifact-inspection'
import { LibraryNewLoadout } from './library-new-loadout'
import { LibraryTabs } from '@/components/depot/depot-workspace-pages'
import { getBrowserSessionEpoch } from '@/lib/auth/session-store'
import { artifactDescription, artifactExportFilename, artifactId, artifactKind, artifactLabel, collectArtifactKinds, collectArtifactTags, filterLibraryArtifacts, sortLibraryArtifacts, serializeArtifact, filterLibraryView, libraryUpdatedLabel, type LibraryView } from './library-model'

const USE_MOCK_DATA = process.env.NEXT_PUBLIC_MOCK_DATA === 'true'
import { ARTIFACT_TYPES, ArtifactTypeMark, artifactTypeDefinition } from './artifact-type'
import { updateLibraryUrl as updateUrl } from './library-url'

type LibraryState = {
  artifacts: DepotArtifact[]
  cursor?: string
  error?: string
  loading: boolean
  canCreate?: boolean
  total?: number
}

type ViewMode = 'table' | 'list' | 'cards'

export function libraryFilterKinds(artifacts: DepotArtifact[]) {
  return [...new Set([...ARTIFACT_TYPES, ...collectArtifactKinds(artifacts)])]
}

export function LibraryFilterRail({ artifacts, kind, onKind, tag, onTag, libraryView, onView, mobileOpen = false, id }: { artifacts: DepotArtifact[]; kind: string; onKind: (kind: string) => void; tag?: string; onTag?: (tag: string | undefined) => void; libraryView?: LibraryView; onView?: (view: LibraryView) => void; mobileOpen?: boolean; id?: string }) {
  return <aside id={id} aria-label="Library filters" data-lbrail="1" className={`${mobileOpen ? 'block' : 'hidden'} min-w-0 self-start min-[901px]:block lg:sticky lg:top-3`}>
    <div className="space-y-3">
    <div className="rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-strong p-[7px] shadow-[var(--aurora-shadow-medium)]" >
      {libraryView && onView ? <>
        {([['all', 'All Artifacts', Box], ['forks', 'Forks', GitFork], ['behind', 'Behind Upstream', RefreshCw], ['published', 'Published', Globe], ['team', 'Team', Users], ['private', 'Private', LockKeyhole]] as const).map(([value, label, Icon]) => <button key={value} type="button" title="Filter loaded library records" aria-pressed={libraryView === value} onClick={() => onView(value)} className="flex w-full items-center gap-[9px] rounded-lg border border-transparent px-[12px] text-left font-semibold text-aurora-text-muted hover:bg-aurora-hover-bg aria-pressed:border-aurora-border-strong aria-pressed:bg-aurora-selected-bg aria-pressed:text-aurora-text-primary" style={{ height: 34, fontSize: 13 }}><Icon size={15}/><span className="min-w-0 flex-1">{label}</span><span className="text-[10.5px] tabular-nums opacity-75">{filterLibraryView(artifacts, value).length}</span></button>)}
      </> : <>
      <p className="px-2 pb-2 pt-1 text-[10px] font-semibold text-aurora-text-muted">Artifact types · loaded results</p>
      {['all', ...libraryFilterKinds(artifacts)].map(value => {
        const definition = artifactTypeDefinition(value)
        const Icon = value === 'all' ? Box : definition.icon
        const count = value === 'all' ? artifacts.length : artifacts.filter(artifact => artifactKind(artifact) === value).length
        return <button key={value} type="button" aria-pressed={kind === value} onClick={() => onKind(value)} className="flex min-h-[30px] w-full items-center gap-2 rounded-lg border border-transparent px-2 text-left text-[12.5px] font-semibold text-aurora-text-muted hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary aria-pressed:border-aurora-border-strong aria-pressed:bg-aurora-selected-bg aria-pressed:text-aurora-text-primary">
          <Icon aria-hidden="true" className="size-3.5 shrink-0" /><span className="min-w-0 flex-1 truncate">{value === 'all' ? 'All artifacts' : definition.label}</span><span className="text-[10.5px] tabular-nums opacity-75">{count}</span>
        </button>
      })}
      </>}
    </div>
    <section aria-label="Tags" className="overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-strong" style={{ marginTop: 12 }}>
      <h2 className="flex items-center border-b border-aurora-border-subtle bg-aurora-control-surface font-semibold uppercase text-aurora-text-muted" style={{ minHeight: 36, paddingInline: 12, fontSize: 11, letterSpacing: '.08em' }}>Tags</h2>
      <div className="flex flex-wrap" style={{ gap: 6, padding: '10px 12px' }}>
        {collectArtifactTags(artifacts).map(({ tag: value, count }) => <Button data-visible-label="1" key={value} variant="outline" size="sm" aria-pressed={tag === value} onClick={() => onTag?.(tag === value ? undefined : value)} className="max-w-full gap-[5px] rounded-full aria-pressed:border-aurora-accent-primary aria-pressed:bg-aurora-selected-bg" style={{ height: 24, paddingInline: 8, fontSize: 11 }}><span className="truncate">#{value}</span><span className="tabular-nums text-aurora-text-muted">{count}</span></Button>)}
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
  const [selected, setSelected] = useState<Set<string>>(() => new Set())
  const allSelected = artifacts.length > 0 && artifacts.every(artifact => selected.has(artifactId(artifact)))
  const toggleAll = () => setSelected(allSelected ? new Set() : new Set(artifacts.map(artifactId)))
  const toggleOne = (id: string) => setSelected(current => { const next = new Set(current); if (next.has(id)) next.delete(id); else next.add(id); return next })
  const selectionButton = (checked: boolean, label: string, onClick: () => void) => <button type="button" aria-label={label} aria-pressed={checked} onClick={event => { event.stopPropagation(); onClick() }} className="grid shrink-0 place-items-center rounded-[4px] border focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary" style={{ width: 18, height: 18, borderColor: checked ? 'var(--aurora-accent-primary)' : 'var(--aurora-border-strong)', background: checked ? 'color-mix(in srgb, var(--aurora-accent-primary) 22%, var(--aurora-control-surface))' : 'var(--aurora-control-surface)', color: 'var(--aurora-accent-strong)' }}>{checked ? <Check aria-hidden style={{ width: 12, height: 12 }}/> : null}</button>
  return <div className="aurora-scrollbar overflow-auto" style={{ maxHeight: '55.3vh' }}>
    <table aria-label="Library artifacts" className="w-full min-w-[1120px] table-fixed text-left">
      <colgroup><col className="w-[140px]"/><col className="w-[260px]"/><col className="w-[170px]"/><col className="w-[130px]"/><col className="w-[145px]"/><col className="w-[120px]"/><col className="w-[72px]"/></colgroup>
      <thead className="sticky top-0 z-10 bg-aurora-panel-strong"><tr className="border-b border-aurora-border-subtle font-bold uppercase text-aurora-text-muted" style={{ height: 38, fontSize: 10, letterSpacing: '.1em' }}>
        <th scope="col" className="pl-4 pr-2"><div className="flex items-center" style={{ gap: 16 }}>{selectionButton(allSelected, allSelected ? 'Clear artifact selection' : 'Select all artifacts', toggleAll)}<span>Kind</span></div></th>
        {['Artifact', 'Tags', 'Visibility', 'Upstream', 'Updated'].map(label => <th key={label} scope="col" className="px-2">{label}</th>)}
        <th scope="col"><span className="sr-only">Actions</span></th>
      </tr></thead>
      <tbody>{artifacts.map(artifact => {
        const id = artifactId(artifact)
        const behind = artifact.upstreamBehind ?? 0
        const checked = selected.has(id)
        return <tr key={id} onClick={() => onInspect(id)} className="group cursor-pointer border-b border-aurora-border-subtle/70 last:border-b-0 hover:bg-aurora-surface-muted focus-within:bg-aurora-surface-muted" style={{ height: 46 }}>
          <td className="pl-4 pr-2"><div className="flex items-center" style={{ gap: 12 }}>{selectionButton(checked, `Select ${artifactLabel(artifact)}`, () => toggleOne(id))}<ArtifactTypeMark artifact={artifact} compact/></div></td>
          <td className="px-2"><button type="button" onClick={event => { event.stopPropagation(); onInspect(id) }} aria-label={`Inspect ${artifactLabel(artifact)}`} className="block w-full min-w-0 rounded text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">
            <span title={artifact.namespace ?? artifact.descriptor?.namespace} className="block truncate font-semibold text-aurora-text-primary" style={{ fontSize: 13, lineHeight: '17px' }}>{artifactLabel(artifact)}</span>
            <span className="block truncate text-aurora-text-muted" style={{ fontSize: 11, lineHeight: '15px' }}>{artifactDescription(artifact)}</span>
          </button></td>
          <td className="px-2 text-aurora-text-muted"><div className="flex gap-1.5 overflow-hidden" title={artifact.descriptor?.tags?.join(', ')}>{artifact.descriptor?.tags?.length ? artifact.descriptor.tags.map(tag => <span key={tag} className="max-w-[116px] shrink-0 truncate rounded border border-[color-mix(in_srgb,var(--aurora-accent-primary)_35%,transparent)] bg-[color-mix(in_srgb,var(--aurora-accent-primary)_10%,transparent)] px-[9px] font-[650] text-aurora-accent-strong" style={{ height: 24, lineHeight: '22px', fontSize: 11 }}>#{tag}</span>) : '—'}</div></td>
          <td className="truncate px-2 font-semibold" style={{ fontSize: 12.5 }}><LibraryVisibility visibility={artifact.publication?.visibility}/></td>
          <td className="truncate px-2 text-aurora-text-muted" style={{ fontSize: 12.5 }}>{behind > 0 ? <span className="inline-flex items-center gap-[7px] text-aurora-accent-strong"><span aria-hidden className="size-[6px] rounded-full bg-aurora-accent-primary shadow-[0_0_7px_var(--aurora-accent-primary)]"/><span>{behind} behind</span></span> : <span className="inline-flex items-center gap-[7px]"><span aria-hidden className="size-[5px] rounded-full bg-aurora-text-muted"/><span>Origin</span></span>}</td>
          <td className="px-2 tabular-nums text-aurora-text-muted" style={{ fontSize: 12.5 }}>{libraryUpdatedLabel(artifact)}{libraryUpdatedLabel(artifact) === '—' ? '' : ' ago'}</td>
          <td className="pr-3"><div className="flex items-center justify-end gap-1"><button type="button" aria-label={`Open actions for ${artifactLabel(artifact)}`} title="Open artifact actions" onClick={event => { event.stopPropagation(); onInspect(id) }} className="grid size-7 place-items-center rounded-lg text-aurora-text-muted transition-colors hover:bg-aurora-hover-bg hover:text-aurora-accent-strong focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary"><Send aria-hidden className="size-3.5"/></button><button type="button" aria-label={`Inspect ${artifactLabel(artifact)} details`} title="Inspect artifact details" onClick={event => { event.stopPropagation(); onInspect(id) }} className="grid size-7 place-items-center rounded-lg text-aurora-text-muted transition-colors hover:bg-aurora-hover-bg hover:text-aurora-accent-pink focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary"><Pencil aria-hidden className="size-3.5"/></button></div></td>
        </tr>
      })}</tbody>
    </table>
  </div>
}

export function LibrarySortMenu({ sort, onSort }: { sort: 'catalog' | 'name' | 'kind'; onSort: (sort: 'catalog' | 'name' | 'kind') => void }) {
  const choices = [['catalog', 'Updated'], ['name', 'Name'], ['kind', 'Kind']] as const
  return <div role="group" aria-label="Sort loaded library results" className="flex items-center gap-1 rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low p-0.5">{choices.map(([value, label]) => {
    const active = sort === value
    return <button data-visible-label="1" key={value} type="button" aria-pressed={active} onClick={() => onSort(value)} className="inline-flex items-center justify-center rounded-aurora-1 px-2.5 text-xs font-semibold text-aurora-text-muted transition-colors hover:text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary aria-pressed:bg-aurora-selected-bg aria-pressed:text-aurora-accent-strong" style={{ height: 28 }}>{label}</button>
  })}</div>
}

export function LibraryPageContent() {
  // Library is a user-level hub. Depot already applies authenticated visibility
  // policy and folds live local projections into the catalog, so browsing does
  // not require a project workspace merely to establish an Artifact authority.
  return <SessionLibraryPage />
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
  const [narrowCollection, setNarrowCollection] = useState(false)
  const collectionRef = useRef<HTMLElement | null>(null)
  const [filtersOpen, setFiltersOpen] = useState(false)
  const filtersId = useId()
  const listController = useRef<AbortController | null>(null)

  useEffect(() => {
    const media = window.matchMedia('(max-width: 640px)')
    const applyResponsiveDefault = () => {
      setViewState(media.matches ? 'cards' : 'table')
    }
    applyResponsiveDefault()
    media.addEventListener('change', applyResponsiveDefault)
    return () => media.removeEventListener('change', applyResponsiveDefault)
  }, [])

  useEffect(() => {
    const collection = collectionRef.current
    if (!collection || typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(([entry]) => setNarrowCollection((entry?.contentRect.width ?? 0) < 700))
    observer.observe(collection)
    return () => observer.disconnect()
  }, [])

  const load = useCallback(async (search: string, cursor?: string) => {
    listController.current?.abort()
    const controller = new AbortController()
    listController.current = controller
    const signal = controller.signal
    const loadingSessionEpoch = getBrowserSessionEpoch()
    const isCurrent = () => !signal.aborted && loadingSessionEpoch === getBrowserSessionEpoch()
    setState((current) => cursor
      ? { ...current, loading: true, error: undefined }
      : { artifacts: [], loading: true })
    try {
      const normalizedSearch = search.trim()
      const response = await depotCall<{ result: { artifacts: DepotArtifact[]; nextCursor?: string; total?: number } }>(
        'depot.artifacts.list',
        { limit: 200, ...(normalizedSearch.length >= 3 ? { query: normalizedSearch } : {}), ...(cursor ? { cursor } : {}) },
        signal,
      )
      if (!isCurrent()) return
      const page = response.result
      setState((current) => {
        const artifacts = cursor ? [...current.artifacts, ...page.artifacts] : page.artifacts
        return { artifacts, cursor: page.nextCursor, loading: false, total: page.total ?? artifacts.length }
      })
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
    void depotCall<{ result: { artifact: DepotArtifact } }>('depot.artifacts.get', { artifactId: selectedId }, controller.signal)
      .then((response) => { if (isCurrent()) setDetail(response.result.artifact ? { selectedId, artifact: response.result.artifact } : null) })
      .catch((error) => { if (isCurrent()) toast.error(error instanceof Error ? error.message : String(error)) })
      .finally(() => { if (isCurrent()) setDetailLoading(false) })
    return () => controller.abort()
  }, [selectedId])

  const [libraryView, setLibraryView] = useState<LibraryView>('all')
  const visible = useMemo(() => {
    const needle = activeQuery.trim().toLowerCase()
    const filtered = filterLibraryArtifacts(filterLibraryView(state.artifacts, libraryView), kind, tag)
    const searched = !needle ? filtered : filtered.filter((artifact) => [artifact.name, artifact.title, artifact.description, artifact.namespace, artifact.descriptor?.name, artifact.descriptor?.title, artifact.descriptor?.description, ...(artifact.descriptor?.tags ?? [])].some((value) => value?.toLowerCase().includes(needle)))
    return sortLibraryArtifacts(searched, sort)
  }, [activeQuery, kind, tag, sort, libraryView, state.artifacts])
  const libraryTabCounts = useMemo(() => ({
    ...(state.total === undefined ? {} : { artifacts: state.total }),
    ...(USE_MOCK_DATA ? { loadouts: 4, snippets: 6, tools: 167 } : {}),
  }), [state.total])
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
  const previewDetail = useMemo(() => detail ? localLibraryFederatedArtifact(detail) : null, [detail])
  const previewSpecLabels = useMemo(() => {
    if (!previewDetail) return []
    if (USE_MOCK_DATA) return mockDepotSpecLabels(previewDetail)
    const count = detail?.currentRevision?.fileCount ?? detail?.currentRevision?.components?.length
    return count ? [`${count} files`] : []
  }, [detail, previewDetail])
  const previewMetricLabels = useMemo(() => previewDetail && USE_MOCK_DATA ? mockDepotMetricLabels(previewDetail) : undefined, [previewDetail])
  const effectiveView = narrowCollection ? 'cards' : view

  return <>
    <AppHeader breadcrumbs={[{ label: 'Library' }]} />
    <div data-library-page="1" className={`${AURORA_PAGE_SHELL} min-w-0 flex-1`}><div className={AURORA_PAGE_FRAME} style={{ gap: 16 }}>
      <ConsoleHero actionPresentation="mixed" eyebrow="Depot · Library" title="Library" footer={<LibraryTabs active="artifacts" attached counts={state.error ? {} : libraryTabCounts} />} actions={<div className="flex items-center gap-[9px]"><Button variant="outline" size="icon" aria-label="Export loaded library metadata" title="Export loaded artifact metadata; this does not include artifact content" className="rounded-[10px]" disabled={state.loading || state.artifacts.length === 0} onClick={() => { const url = URL.createObjectURL(new Blob([JSON.stringify({ artifacts: state.artifacts, complete: !state.cursor, total: state.total }, null, 2)], { type: 'application/json' })); const anchor = document.createElement('a'); anchor.href = url; anchor.download = 'library-metadata.json'; anchor.click(); URL.revokeObjectURL(url) }}><Download size={13}/></Button><LibraryNewLoadout/></div>} stats={[
        { label: 'Artifacts', value: state.total ?? '—', suffix: 'in library' },
        { label: 'Forks', value: state.loading ? '—' : filterLibraryView(state.artifacts, 'forks').length, suffix: 'tracking upstream', tone: 'var(--aurora-accent-pink)' },
        { label: 'Behind', value: state.loading ? '—' : filterLibraryView(state.artifacts, 'behind').length, suffix: 'need a merge', tone: 'var(--aurora-accent-strong)' },
        { label: 'Public', value: state.loading ? '—' : filterLibraryView(state.artifacts, 'published').length, suffix: 'published', tone: 'var(--aurora-success)' },
        { label: 'Loadouts', value: state.loading ? '—' : state.artifacts.filter((artifact) => artifactKind(artifact) === 'loadout').length, suffix: 'bundled' },
      ]}/>
      {state.error ? <DashboardPanel title="Library unavailable"><div className="flex flex-wrap items-center justify-between gap-3"><div><p role="alert" className="text-sm text-aurora-error">{state.error}</p><p className="mt-1 text-sm text-aurora-text-muted">The catalog could not be loaded. Try again after the connection is restored.</p></div><Button variant="outline" disabled={state.loading} onClick={() => void load(activeQuery)}><RefreshCw className="size-4"/>Retry loading</Button></div></DashboardPanel> : null}
      <div data-lbgrid="1" className="grid min-w-0 items-start max-[900px]:grid-cols-1" style={{ gridTemplateColumns: '220px minmax(0,1fr)', gap: 16 }}>
      <LibraryFilterRail id={filtersId} mobileOpen={filtersOpen} libraryView={libraryView} onView={setLibraryView} artifacts={state.artifacts} kind={kind} onKind={next => { setKind(next); updateUrl({ kind: next }) }} tag={tag} onTag={next => setTagSelection({ query, tag: next })} />
      <div className="min-w-0">
      <section ref={collectionRef} aria-label="Artifact collection" data-library-collection="1" className="overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-strong shadow-[var(--aurora-shadow-medium)]"><div data-library-toolbar="1" className="flex flex-wrap items-center gap-[9px] border-b border-aurora-border-subtle bg-aurora-control-surface" style={{ padding: '9px 12px' }}><div data-library-search="1" className="flex h-9 min-w-32 max-w-[360px] flex-1 items-center rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low focus-within:border-aurora-border-strong"><Search aria-hidden="true" className="ml-3 size-4 shrink-0 text-aurora-text-muted"/><Input aria-label="Search library" className="h-full min-w-0 flex-1 border-0 bg-transparent px-2 text-[13px] shadow-none focus-visible:ring-0" placeholder={`Filter ${state.total ?? state.artifacts.length} artifacts…`} value={query} onChange={(event) => setQuery(event.target.value)}/>{query ? <button type="button" aria-label="Clear library search" className="grid size-7 shrink-0 place-items-center text-aurora-text-muted hover:text-aurora-text-primary" onClick={() => setQuery('')}><X className="size-3.5"/></button> : null}<button type="button" aria-label="Toggle library filters" aria-expanded={filtersOpen} aria-controls={filtersId} aria-pressed={filtersOpen} title="Filters" onClick={() => setFiltersOpen(open => !open)} className="mr-1 grid size-7 shrink-0 place-items-center rounded-aurora-1 text-aurora-text-muted transition-colors hover:bg-aurora-hover-bg hover:text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary aria-pressed:bg-aurora-selected-bg aria-pressed:text-aurora-accent-strong min-[901px]:hidden"><Filter className="size-3.5"/></button></div><span className="font-semibold tabular-nums text-aurora-text-muted" style={{ fontSize: 12, marginLeft: 6 }}>{visible.length} of {state.total ?? state.artifacts.length}</span><div className="flex-1"/><LibrarySortMenu sort={sort} onSort={setSort}/></div>
        {effectiveView !== 'table' ? <div className={effectiveView === 'cards' ? 'grid gap-3 p-3' : 'divide-y divide-aurora-border-subtle'} style={effectiveView === 'cards' ? { gridTemplateColumns: 'repeat(auto-fit, minmax(min(100%, 260px), 1fr))' } : undefined}>{visible.map((artifact) => { const id = artifactId(artifact); return <button key={id} type="button" onClick={() => updateUrl({ artifact: id })} className={effectiveView === 'cards' ? 'group rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-low p-4 text-left transition-[transform,border-color] hover:-translate-y-0.5 hover:border-aurora-border-strong' : 'group flex w-full items-start gap-3 px-3 py-3 text-left transition-colors hover:bg-aurora-surface-muted'}><ArtifactTypeMark artifact={artifact} compact/><span className="min-w-0 flex-1"><span className="block truncate font-semibold text-aurora-text-primary">{artifactLabel(artifact)}</span><span className="mt-1 line-clamp-2 block text-xs leading-5 text-aurora-text-muted">{artifactDescription(artifact)}</span><span className="mt-2 block truncate text-[11px] text-aurora-text-muted">{artifact.namespace ?? artifact.descriptor?.namespace ?? 'Unknown namespace'}</span></span><ChevronRight className="mt-1 size-4 shrink-0 text-aurora-text-muted group-hover:text-aurora-accent-primary"/></button> })}</div> : <LibraryArtifactTable artifacts={visible} onInspect={id => updateUrl({ artifact: id })}/>}
        {state.loading && state.artifacts.length === 0 ? <p className="flex items-center justify-center gap-2 py-10 text-sm text-aurora-text-muted"><Loader2 className="size-4 animate-spin"/>Loading the Labby catalog…</p> : null}
        {!state.loading && !state.error && visible.length === 0 ? <p className="py-10 text-center text-sm text-aurora-text-muted">{state.artifacts.length ? 'No artifacts match these filters.' : 'No artifacts in your library yet. Explore Discover to add one.'}</p> : null}
        {state.cursor ? <div className="border-t border-aurora-border-subtle p-3 text-center"><Button variant="outline" disabled={state.loading} onClick={() => void load(activeQuery, state.cursor)}>{state.loading ? <Loader2 className="size-4 animate-spin"/> : null}Load more</Button></div> : null}
      </section>
      <div aria-label="Library connection" className="mt-2 flex items-center gap-2 text-[10.5px] text-aurora-text-muted"><span>{state.loading ? 'Loading Library…' : state.error ? 'Library unavailable' : 'Depot catalog + live local projections'}</span><button type="button" className="ml-auto rounded px-1.5 hover:text-aurora-accent-strong" disabled={state.loading} onClick={() => void load(activeQuery)}>Refresh</button><a className="hover:text-aurora-accent-strong" href="/depot">Discover</a></div>
      </div>
      </div>
    </div></div>
    <DiscoverArtifactInspection
      artifact={previewDetail}
      previewMode
      specLabels={previewSpecLabels}
      metricLabels={previewMetricLabels}
      inLibrary
      loading={detailLoading}
      open={Boolean(selectedId)}
      copied={copied}
      importing={false}
      onImport={async () => undefined}
      onFork={(artifact) => { window.location.href = `/depot?artifact=${encodeURIComponent(artifact.artifactId)}` }}
      onSend={async () => { await shareArtifact() }}
      installFormats={['JSON']}
      onInstallFormat={() => { if (detail) exportArtifact(detail) }}
      onOpenChange={(open) => { if (!open) updateUrl({ artifact: null }) }}
      onCopy={(label, value) => { if (value) void copy(label, value) }}
      onExport={() => { if (detail) exportArtifact(detail) }}
    />
  </>
}
