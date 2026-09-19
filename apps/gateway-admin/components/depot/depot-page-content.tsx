'use client'

import { useCallback, useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from 'react'
import { usePathname, useRouter, useSearchParams } from 'next/navigation'
import Link from 'next/link'
import { ArrowLeftRight, Compass, Info, Loader2, Plus } from 'lucide-react'
import { toast } from 'sonner'

import { AppHeader } from '@/components/app-header'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { Button } from '@/components/ui/button'
import { DiscoverResultTabs } from './discover-result-tabs'
import { DiscoverFilterPanel, DiscoverSearchControls, type DiscoveryVisibility } from './discover-search-controls'
import { mockDepotArtifacts, mockDepotLibraryArtifactIds, mockDepotMetricLabels, mockDepotNow, mockDepotSpecLabels } from '@/lib/api/depot-mock-data'
import { getArtifact, listArtifacts, listProviderOptions, type DepotArtifact, type DepotProviderOption, type FederatedArtifact } from '@/lib/api/depot-client'
import { getBrowserSessionEpoch, subscribeToBrowserSession } from '@/lib/auth/session-store'
import { artifactKey } from '@/lib/depot/provider-model'
import { appendDiscoveryPage, createDiscoveryWindow, visibleArtifacts, type DiscoveryWindow } from './discovery-window'
import { RequestLanes } from './request-lanes'
import { controlPlaneAction } from '@/lib/api/artifact-control-client'
import { artifactMatchLabel, DISCOVERY_SHELVES, selectDiscoveryResults, selectDiscoveryShelf, type DiscoveryShelf, type DiscoverySort } from './discover-model'
import { DiscoverArtifactCard as ArtifactCard } from './discover-artifact-card'
import { DiscoverArtifactInspection as ArtifactInspection } from './discover-artifact-inspection'
import { DiscoverViewOptions, type DiscoveryDensity, type DiscoveryLayout } from './discover-view-options'
import { DiscoverRails } from './discover-rails'
import { canCompareBundles, DiscoverBundleCompare } from './discover-bundle-compare'

const USE_MOCK_DATA = process.env.NEXT_PUBLIC_MOCK_DATA === 'true'

type LoadState = { deferredAttempts?: number; failures?: string[]; loading: boolean; error?: string; window: DiscoveryWindow; cursor?: string; total?: number; exact: boolean; coverage?: string; scopeEpoch?: string }
export function discoveryCountLabel(count: number, exact: boolean, unavailable: boolean) {
  return unavailable ? '—' : `${exact ? '' : '≥ '}${count.toLocaleString()}`
}

export function depotCoveragePulse(coverage?: string, error?: string) {
  if (error || coverage === 'all_failed') {
    return { color: 'var(--aurora-error)', label: coverage ?? 'unavailable' }
  }
  if (coverage === 'partial' || coverage === 'deferred' || coverage === 'all_disabled') {
    return { color: 'var(--aurora-warn)', label: coverage }
  }
  return { color: 'var(--aurora-success)', label: coverage ?? 'ready' }
}

export function mergeArtifactPages(current: DepotArtifact[], incoming: DepotArtifact[]): DepotArtifact[] {
  const seen = new Set(current.map(artifact => artifact.id ?? artifact.descriptor?.id).filter(Boolean))
  return [...current, ...incoming.filter(artifact => {
    const id = artifact.id ?? artifact.descriptor?.id
    if (!id || seen.has(id)) return false
    seen.add(id)
    return true
  })]
}

export function exactImportConnection(providerId: string, connections: Array<{ id: string }>): string {
  const match = connections.find(connection => connection.id === providerId)
  if (match) return match.id
  throw new Error(`Configure an Artifact acquisition connection named “${providerId}” before importing from this provider.`)
}

export function exactImportParams(
  artifact: Pick<FederatedArtifact, 'providerId' | 'artifactId' | 'id' | 'currentRevisionId' | 'currentRevision'>,
  connections: Array<{ id: string }>,
  libraryVersion: unknown,
  idempotencyKey: string,
) {
  const artifactId = artifact.artifactId || artifact.id
  const revisionId = artifact.currentRevisionId || artifact.currentRevision?.id
  if (!artifactId || !revisionId) throw new Error('Labby catalog did not provide an exact Artifact and revision identity.')
  const connectionId = exactImportConnection(artifact.providerId, connections)
  if (typeof libraryVersion !== 'number' || !Number.isSafeInteger(libraryVersion) || libraryVersion < 0) {
    throw new Error('Labby did not return a valid current library version.')
  }
  return {
    source: { kind: 'depot' as const, connection_id: connectionId, artifact_id: artifactId, revision_id: revisionId },
    expected_library_version: libraryVersion,
    idempotency_key: idempotencyKey,
  }
}
export function DepotPageContent() {
  const sessionEpoch = useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionEpoch, () => 0)
  return <SessionDepotPage key={sessionEpoch} />
}

function SessionDepotPage() {
  const router = useRouter(), pathname = usePathname(), searchParams = useSearchParams()
  const selectedId = searchParams.get('artifact') ?? undefined, selectedArtifactProvider = searchParams.get('artifactProvider') ?? undefined
  const selectedProvider = searchParams.get('provider') ?? 'all', initialQuery = searchParams.get('q')?.trim() ?? ''
  const kind = searchParams.get('kind') ?? 'all'
  const [query,setQuery] = useState(initialQuery), [activeQuery,setActiveQuery] = useState(initialQuery)
  const [state,setState] = useState<LoadState>({ loading:true, window:createDiscoveryWindow(), exact:false })
  const [providers,setProviders] = useState<DepotProviderOption[]>([])
  const [providerError,setProviderError] = useState<string>()
  const [detail,setDetail] = useState<FederatedArtifact|null>(null), [detailLoading,setDetailLoading] = useState(false)
  const [copied,setCopied] = useState<string>(), [view,setView] = useState<DiscoveryLayout>('cards')
  const [importing,setImporting] = useState(false)
  const importPending = useRef(false)
  const [density, setDensity] = useState<DiscoveryDensity>('comfortable')
  const [shelf, setShelf] = useState<DiscoveryShelf>('trending')
  const [visibility, setVisibility] = useState<DiscoveryVisibility>('all')
  const [filtersOpen, setFiltersOpen] = useState(false)
  const [selectionMode, setSelectionMode] = useState(false)
  const [bulkSelectedKeys, setBulkSelectedKeys] = useState<string[]>([])
  const [cursorIndex, setCursorIndex] = useState(-1)
  const [compareOpen, setCompareOpen] = useState(false)
  const [now, setNow] = useState<number>()
  const lanes = useRef(new RequestLanes()), inFlight = useRef<string | undefined>(undefined)
  const loadMoreRef = useRef<HTMLDivElement>(null)
  const paginationControllerRef = useRef<AbortController>(null)
  const detailControllerRef = useRef<AbortController>(null)
  const contextKey = JSON.stringify([selectedProvider, kind, query.trim()])
  const contextRef = useRef(contextKey)
  const previousUrlQuery = useRef(initialQuery)
  const invalidateContext = useCallback((key: string) => {
    contextRef.current = key
    lanes.current.invalidateContext()
    paginationControllerRef.current?.abort()
    detailControllerRef.current?.abort()
    inFlight.current = undefined
    setDetail(null)
    setDetailLoading(false)
    setBulkSelectedKeys([])
    setSelectionMode(false)
    setCompareOpen(false)
    setCursorIndex(-1)
    setState({ loading: true, window: createDiscoveryWindow(), exact: false })
  }, [])
  useLayoutEffect(() => {
    if (previousUrlQuery.current !== initialQuery) {
      previousUrlQuery.current = initialQuery
      if (query.trim() !== initialQuery) {
        invalidateContext(JSON.stringify([selectedProvider, kind, initialQuery]))
        setQuery(initialQuery)
        return
      }
    }
    if (contextRef.current !== contextKey) invalidateContext(contextKey)
  }, [contextKey, initialQuery, invalidateContext, kind, query, selectedProvider])
  const changeFilter = useCallback((field: 'kind' | 'provider', value: string) => {
    // Re-selecting an active filter must preserve any in-flight catalog request.
    if (value === (field === 'provider' ? selectedProvider : kind)) return
    invalidateContext(JSON.stringify([field === 'provider' ? value : selectedProvider, field === 'kind' ? value : kind, query.trim()]))
    const params = new URLSearchParams(window.location.search)
    if (value === 'all') params.delete(field)
    else params.set(field, value)
    if (query.trim()) params.set('q', query.trim())
    else params.delete('q')
    params.delete('artifact')
    params.delete('artifactProvider')
    router.replace(`${pathname}${params.size ? `?${params}` : ''}`, { scroll: false })
  }, [invalidateContext, kind, pathname, query, router, selectedProvider])

  useEffect(() => {
    if (USE_MOCK_DATA) {
      setNow(mockDepotNow)
      return
    }
    const refresh = () => setNow(Date.now())
    refresh()
    const timer = window.setInterval(refresh, 60_000)
    return () => window.clearInterval(timer)
  }, [])

  const load = useCallback(async (searchQuery:string,cursor?:string,signal?:AbortSignal) => {
    if (JSON.stringify([selectedProvider, kind, searchQuery]) !== contextRef.current) return
    const key = JSON.stringify([selectedProvider,kind,searchQuery,cursor??null])
    if(inFlight.current===key)return
    const generation = lanes.current.begin('list')
    const epoch = getBrowserSessionEpoch()
    const isCurrent = () => epoch === getBrowserSessionEpoch() && lanes.current.isCurrent('list', generation) && !signal?.aborted
    inFlight.current=key
    setState(c=>({...c,loading:true,error:undefined,deferredAttempts:cursor?c.deferredAttempts:0,window:cursor?c.window:createDiscoveryWindow(),cursor:cursor?c.cursor:undefined,total:cursor?c.total:undefined}))
    try {
      const listing = await listArtifacts({provider:selectedProvider,query:searchQuery,kind,limit:50,cursor},signal)
      if(!isCurrent())return
      setState(c=>({deferredAttempts:listing.state==='deferred'?(cursor?(c.deferredAttempts??0)+1:0):0,loading:false,window:appendDiscoveryPage(cursor?c.window:createDiscoveryWindow(),listing.items),cursor:listing.nextCursor??undefined,total:listing.knownTotal??undefined,exact:listing.totalIsExact,coverage:listing.state,scopeEpoch:listing.scopeEpoch,failures:listing.failures.map(failure=>failure.kind)}))
    } catch(error) { if(isCurrent())setState(c=>({...c,loading:false,error:error instanceof Error?error.message:String(error)})) }
    finally { if(lanes.current.isCurrent('list',generation))inFlight.current=undefined }
  },[selectedProvider,kind])

  useEffect(()=>{const controller=new AbortController();const epoch=getBrowserSessionEpoch();void listProviderOptions(controller.signal).then(value=>{if(!controller.signal.aborted&&epoch===getBrowserSessionEpoch()){setProviders(value);setProviderError(undefined)}}).catch(error=>{if(!controller.signal.aborted&&epoch===getBrowserSessionEpoch())setProviderError(error instanceof Error?error.message:String(error))});return()=>controller.abort()},[])
  useEffect(()=>()=>{lanes.current.invalidateContext();paginationControllerRef.current?.abort();detailControllerRef.current?.abort()},[])

  useEffect(()=>{ const controller=new AbortController(); const timer=window.setTimeout(()=>{ const next=query.trim(); setActiveQuery(next); const params=new URLSearchParams(window.location.search); if((params.get('q')?.trim()??'')!==next){if(next)params.set('q',next);else params.delete('q');params.delete('artifact');params.delete('artifactProvider');router.replace(`${pathname}${params.size?`?${params}`:''}`,{scroll:false})} if(next.length===0||next.length>=3)void load(next,undefined,controller.signal);else setState(current=>({...current,loading:false,error:undefined,window:createDiscoveryWindow(),cursor:undefined,total:undefined,exact:false}))},query?300:0); return()=>{window.clearTimeout(timer);controller.abort()} },[load,pathname,query,router])
  useEffect(()=>{lanes.current.invalidate('import');const generation=lanes.current.begin('detail');const epoch=getBrowserSessionEpoch();if(!selectedId||!selectedArtifactProvider||query.trim()!==initialQuery||contextRef.current!==contextKey){setDetail(null);setDetailLoading(false);return}const controller=new AbortController();detailControllerRef.current=controller;setDetail(null);setDetailLoading(true);void getArtifact(selectedArtifactProvider,selectedId,controller.signal).then(r=>{if(lanes.current.isCurrent('detail',generation)&&!controller.signal.aborted&&epoch===getBrowserSessionEpoch())setDetail({...r.artifact,providerId:r.providerId,artifactId:r.artifactId})}).catch(e=>{if(lanes.current.isCurrent('detail',generation)&&!controller.signal.aborted&&epoch===getBrowserSessionEpoch())toast.error(e instanceof Error?e.message:String(e))}).finally(()=>{if(lanes.current.isCurrent('detail',generation)&&!controller.signal.aborted&&epoch===getBrowserSessionEpoch())setDetailLoading(false)});return()=>controller.abort()},[contextKey,initialQuery,query,selectedArtifactProvider,selectedId])
  useEffect(() => {
    if (!state.cursor || state.loading || state.error) return
    const cursor = state.cursor
    const controller = new AbortController()
    const continueListing = () => {
      paginationControllerRef.current = controller
      void load(activeQuery, cursor, controller.signal)
    }
    // Busy providers return continuations without rows. A visible sentinel must not
    // turn that response into an unbounded request loop. Keep completed rows while
    // retrying, then leave an explicit user-controlled continuation after five tries.
    if (state.coverage === 'deferred') {
      const attempt = state.deferredAttempts ?? 0
      if (attempt >= 5) return
      const timer = window.setTimeout(continueListing, 500 * 2 ** attempt)
      return () => window.clearTimeout(timer)
    }
    const target = loadMoreRef.current
    if (!target) return
    const observer = new IntersectionObserver(entries => {
      if (entries.some(entry => entry.isIntersecting)) {
        observer.disconnect()
        continueListing()
      }
    }, { rootMargin: '600px 0px' })
    observer.observe(target)
    return () => observer.disconnect()
  }, [activeQuery, load, state.cursor, state.error, state.loading, state.coverage, state.deferredAttempts])

  const artifactHref=useCallback((providerId?:string,id?:string)=>{const params=new URLSearchParams();if(activeQuery)params.set('q',activeQuery);if(kind!=='all')params.set('kind',kind);if(selectedProvider!=='all')params.set('provider',selectedProvider);if(providerId&&id){params.set('artifactProvider',providerId);params.set('artifact',id)}return `${pathname}${params.size?`?${params}`:''}`},[activeQuery,kind,pathname,selectedProvider])
  const resetDiscovery=useCallback(()=>{window.history.replaceState(window.history.state,'',pathname);invalidateContext(JSON.stringify(['all','all','']));setQuery('');setActiveQuery('');setVisibility('all');setShelf('trending');setSort('relevance');setFiltersOpen(false);setBulkSelectedKeys([]);setSelectionMode(false);setCompareOpen(false);setCursorIndex(-1);router.replace(pathname,{scroll:false})},[invalidateContext,pathname,router])
  const copyValue=useCallback(async(label:string,value?:string)=>{if(!value)return;await navigator.clipboard.writeText(value);setCopied(label);toast.success(`${label} copied`);window.setTimeout(()=>setCopied(c=>c===label?undefined:c),1500)},[])
  const exportArtifact=useCallback((artifact:FederatedArtifact)=>{const label=artifact.name??artifact.descriptor?.name??artifact.kind??'artifact';const blob=new Blob([`${JSON.stringify(artifact,null,2)}\n`],{type:'application/json'}),url=URL.createObjectURL(blob),anchor=document.createElement('a');anchor.href=url;anchor.download=`${label.toLowerCase().replace(/[^a-z0-9._-]+/g,'-')}.depot.json`;anchor.click();URL.revokeObjectURL(url);toast.success('Artifact metadata exported')},[])
  const importArtifact=useCallback(async(artifact:FederatedArtifact)=>{
    if(importPending.current)return
    const epoch=getBrowserSessionEpoch()
    const generation=lanes.current.begin('import')
    const isCurrent=()=>epoch===getBrowserSessionEpoch()&&lanes.current.isCurrent('import',generation)
    const artifactId=artifact.artifactId||artifact.id
    const revisionId=artifact.currentRevisionId||artifact.currentRevision?.id
    if(!artifactId||!revisionId)throw new Error('Labby catalog did not provide an exact Artifact and revision identity.')
    importPending.current=true
    setImporting(true)
    try{
      const artifactId=artifact.artifactId||artifact.id
      const revisionId=artifact.currentRevisionId||artifact.currentRevision?.id
      if(!artifactId||!revisionId)throw new Error('Labby catalog did not provide an exact Artifact and revision identity.')
      const [connectionResult,libraryResult]=await Promise.all([
        controlPlaneAction<{connections?:Array<{id:string}>}>('artifacts','artifacts.list_connections'),
        controlPlaneAction<{library_version?:number}>('artifacts','artifacts.list',{limit:1}),
      ])
      if(!isCurrent())return
      const importParams=exactImportParams(
        artifact,
        connectionResult.connections??[],
        libraryResult.library_version,
        `depot-import-${crypto.randomUUID()}`,
      )
      await controlPlaneAction('artifacts','artifacts.import',importParams)
      if(isCurrent())toast.success('Exact Artifact imported into Labby')
    }catch(error){if(isCurrent())toast.error(error instanceof Error?error.message:String(error))}
    finally{importPending.current=false;setImporting(false)}
  },[])
  const addArtifactToLibrary=useCallback(async(artifact:FederatedArtifact)=>{
    if(USE_MOCK_DATA){
      toast.success(`Preview: ${artifact.name??artifact.title??artifact.artifactId} would be added to Library`)
      return
    }
    await importArtifact(artifact)
  },[importArtifact])
  const previewFork=useCallback((artifact:FederatedArtifact)=>{toast.success(`Forked ${artifact.name??artifact.title??artifact.artifactId} — preview only`)},[])
  const previewSend=useCallback((artifact:FederatedArtifact)=>{toast.success(`${artifact.name??artifact.title??artifact.artifactId} sent to Labby — preview only`)},[])
  const previewInstallFormat=useCallback((artifact:FederatedArtifact,format:string)=>{toast.success(`${artifact.name??artifact.title??artifact.artifactId} → ${format} · preview only`)},[])
  const isArtifactInLibrary=useCallback((artifact:FederatedArtifact)=>USE_MOCK_DATA&&mockDepotLibraryArtifactIds.has(artifact.artifactId),[])
  const visible = visibleArtifacts(state.window)
  const [sort, setSort] = useState<DiscoverySort>('relevance')
  const visibilityResults = visibility === 'all' ? visible.items : visible.items.filter(artifact => artifact.publication?.visibility?.toLowerCase() === visibility)
  const shelfResults = USE_MOCK_DATA ? selectDiscoveryShelf(visibilityResults, shelf) : visibilityResults
  const results = selectDiscoveryResults(shelfResults, sort)
  const shelfMeta = DISCOVERY_SHELVES.find(item => item.id === shelf) ?? DISCOVERY_SHELVES[0]
  const selectedBulkArtifacts = results.filter(artifact => bulkSelectedKeys.includes(artifactKey(artifact.providerId, artifact.artifactId)))
  const compareEligible = canCompareBundles(selectedBulkArtifacts)
  const toggleBulkSelection = (artifact: FederatedArtifact) => {
    const key = artifactKey(artifact.providerId, artifact.artifactId)
    setSelectionMode(true)
    setBulkSelectedKeys(current => current.includes(key) ? current.filter(item => item !== key) : [...current, key])
  }
  const enterSelectionMode = (artifact: FederatedArtifact) => {
    const key = artifactKey(artifact.providerId, artifact.artifactId)
    setSelectionMode(true)
    setBulkSelectedKeys(current => current.includes(key) ? current : [...current, key])
  }
  const clearBulkSelection = () => { setBulkSelectedKeys([]); setSelectionMode(false); setCompareOpen(false) }
  const bulkAddToLibrary = async () => {
    const chosen = [...selectedBulkArtifacts]
    if (!chosen.length) return
    if (USE_MOCK_DATA) {
      toast.success(`Preview: ${chosen.length} artifact${chosen.length === 1 ? '' : 's'} would be added to Library`)
      clearBulkSelection()
      return
    }
    for (const artifact of chosen) await importArtifact(artifact)
    clearBulkSelection()
  }
  const resultCount=state.total??state.window.rowCount
  const incomplete = Boolean(state.error) || (state.coverage !== undefined && state.coverage !== 'complete' && state.coverage !== 'empty')
  const incompleteMessage = state.failures?.includes('unsupported_kind')
    ? 'Some sources do not support this kind filter. Results cover the supported sources only.'
    : 'Some sources are still preparing search results or are unavailable. Retry to check again.'

  useEffect(() => {
    if (cursorIndex >= results.length) setCursorIndex(results.length ? results.length - 1 : -1)
  }, [cursorIndex, results.length])

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null
      if (target?.closest('input, textarea, select, [contenteditable="true"]')) return
      if (selectedId || compareOpen || results.length === 0) return
      const down = event.key === 'j' || event.key === 'ArrowDown'
      const up = event.key === 'k' || event.key === 'ArrowUp'
      if (down || up) {
        event.preventDefault()
        const next = down ? Math.min(results.length - 1, cursorIndex < 0 ? 0 : cursorIndex + 1) : Math.max(0, cursorIndex < 0 ? 0 : cursorIndex - 1)
        setCursorIndex(next)
        window.requestAnimationFrame(() => document.querySelectorAll<HTMLElement>('[data-discover-result]')[next]?.scrollIntoView({ block: 'nearest' }))
        return
      }
      if (event.key === 'Enter' && cursorIndex >= 0) {
        event.preventDefault()
        const artifact = results[cursorIndex]
        if (artifact) router.push(artifactHref(artifact.providerId, artifact.artifactId), { scroll: false })
        return
      }
      if (event.key === 'Escape' && cursorIndex >= 0) { event.preventDefault(); setCursorIndex(-1) }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [artifactHref, compareOpen, cursorIndex, results, router, selectedId])

  return <>
    <AppHeader icon={<Compass className="size-3.5" />} breadcrumbs={[{label:'Discover'}]}/>
    <div className={`${AURORA_PAGE_SHELL} flex-1`}><div className={AURORA_PAGE_FRAME} style={{ gap: 14 }}>
      <ConsoleHero variant="discover" icon={<Compass className="size-[22px]" />} eyebrow="Depot · Bazaar" title="Discover" description="Every artifact Depot can reach — registries, marketplaces, catalogs and crawls — searched semantically and installable in any target format through APM." pulse={USE_MOCK_DATA && !state.error ? { color: 'var(--aurora-success)', label: `${providers.filter(provider => provider.enabled).length} sources indexed` } : depotCoveragePulse(state.coverage,state.error)} actions={<Button asChild size="icon" variant="outline" className="size-9 rounded-[10px] text-aurora-accent-strong" style={{ borderColor: 'color-mix(in srgb, var(--aurora-accent-primary) 55%, var(--aurora-border-strong))', background: 'color-mix(in srgb, var(--aurora-accent-primary) 9%, var(--aurora-panel-strong))' }}><Link href="/create" aria-label="Publish artifact" title="Publish artifact"><Plus aria-hidden="true" className="size-[15px]" /></Link></Button>}
        stats={[
          { label: activeQuery ? 'Matches' : 'Indexed', value: discoveryCountLabel(resultCount, state.exact, Boolean(state.error) || state.total === undefined), suffix: 'artifacts' },
          { label: 'Sources', value: providerError && providers.length === 0 ? '—' : providers.filter(provider => provider.enabled).length, suffix: providerError ? 'provider status unavailable' : 'registries + crawls' },
          USE_MOCK_DATA
            ? { label: 'Last crawl', value: '4m', suffix: 'ago', tone: 'var(--aurora-accent-strong)' }
            : { label: 'Last crawl', value: <span title="Crawl timestamps are not reported by the connected sources." className="text-sm font-normal text-aurora-text-muted">Not reported</span> },
          USE_MOCK_DATA
            ? { label: 'Verified', value: visible.items.filter(artifact => artifact.publisherVerified === true).length, suffix: 'publishers', tone: 'var(--aurora-success)' }
            : { label: 'Verified', value: <span title="Publisher verification is not reported consistently by the connected sources." className="text-sm font-normal text-aurora-text-muted">Not reported</span> },
        ]}>
        <DiscoverSearchControls query={query} providers={providers} artifacts={visible.items} kind={kind} selectedProvider={selectedProvider} onFilter={changeFilter} visibility={visibility} onVisibility={setVisibility} filtersOpen={filtersOpen} onFiltersOpenChange={setFiltersOpen} totalCount={USE_MOCK_DATA ? mockDepotArtifacts.length : resultCount} onClearAll={resetDiscovery} showVisibility={!USE_MOCK_DATA} onQuery={next => {
          invalidateContext(JSON.stringify([selectedProvider, kind, next.trim()]))
          setQuery(next)
          setActiveQuery(next.trim())
          const params = new URLSearchParams(window.location.search)
          params.delete('artifact'); params.delete('artifactProvider')
          if (selectedId) router.replace(`${pathname}${params.size ? `?${params}` : ''}`, { scroll: false })
        }} />
      </ConsoleHero>
      <div data-dqgrid="1" className="mt-[-4px] grid min-w-0 grid-cols-[minmax(0,1fr)] items-start gap-[14px]">
      <div className="flex min-w-0 flex-col gap-3">
      <DiscoverFilterPanel open={filtersOpen} providers={providers} artifacts={visible.items} kind={kind} selectedProvider={selectedProvider} onFilter={changeFilter} visibility={visibility} onVisibility={setVisibility} mockKinds={USE_MOCK_DATA} showVisibility={!USE_MOCK_DATA}/>
      {providerError?<DashboardPanel title="Source filter status unavailable"><p role="status" className="text-sm text-aurora-text-muted">Artifact discovery remains usable, but the source selector may be incomplete or stale: {providerError}</p></DashboardPanel>:null}
      {incomplete&&!state.loading?<DashboardPanel title="Search coverage incomplete"><p role="status" className="text-sm text-aurora-text-muted">{incompleteMessage}</p><Button variant="outline" onClick={()=>void load(query.trim())}>Retry search</Button></DashboardPanel>:null}
      {!activeQuery && kind === 'all' && selectedProvider === 'all' && visibility === 'all' && !state.loading ? <DiscoverRails artifacts={USE_MOCK_DATA?visible.items:[]} artifactHref={artifactHref} unavailableReason={USE_MOCK_DATA?undefined:'Recommendation evidence is not reported by the current Depot contract.'}/> : null}
      <section aria-labelledby="artifact-results-title" className="contents">
        <div className="flex flex-wrap items-end gap-[10px] border-b border-aurora-border-default/55 px-0.5">
          <DiscoverResultTabs shelf={shelf} setShelf={setShelf} />
          <span className="min-w-3 flex-1" />
          <span className="mb-[5px] inline-flex h-[22px] items-center rounded-md border border-aurora-border-default/50 bg-aurora-control-surface px-[9px] text-[10.5px] font-[650] tabular-nums text-aurora-text-muted" title={`${state.window.rowCount} retained results; sorting applies to loaded artifacts.`}>{state.loading ? 'Searching…' : incomplete && state.total === undefined ? 'Total unavailable' : `${results.length} shown · ${state.exact ? '' : '≥ '}${resultCount}`}</span>
          <div className="pb-1"><DiscoverViewOptions sort={sort} setSort={setSort} layout={view} setLayout={setView} density={density} setDensity={setDensity} /></div>
        </div>
        <div className="-mt-[5px] flex h-[17px] min-w-0 items-center gap-[7px] px-[3px]"><Info aria-hidden className="size-3 shrink-0 text-[color-mix(in_srgb,var(--aurora-accent-strong)_80%,transparent)]" strokeWidth={1.7}/><span className="shrink-0 font-display text-xs font-bold leading-[17px] tracking-[-0.005em] text-[color-mix(in_srgb,var(--aurora-text-muted)_55%,var(--aurora-text-primary))]">{shelfMeta.title}</span><span aria-hidden className="h-[11px] w-px shrink-0 bg-aurora-border-default/65"/><span className="min-w-0 text-[11.5px] leading-[17px] text-aurora-text-muted">{shelfMeta.hint}</span></div>
        {!USE_MOCK_DATA?<div data-discovery-feed-unavailable className="rounded-aurora-1 border border-dashed border-aurora-border-strong/60 bg-aurora-panel-medium px-3 py-2 text-[11.5px] leading-relaxed text-aurora-text-muted">Depot does not currently report a canonical {shelfMeta.label.toLowerCase()} feed. Results below remain the retained catalog window and are only ordered by the selected display sort.</div>:null}
        {selectedBulkArtifacts.length ? <div className="flex flex-wrap items-center gap-[9px] rounded-aurora-1 border border-[color-mix(in_srgb,var(--aurora-accent-primary)_40%,transparent)] bg-[color-mix(in_srgb,var(--aurora-accent-primary)_8%,var(--aurora-panel-strong))] px-[13px] py-[9px] shadow-[inset_0_1px_0_rgba(255,255,255,0.04)]"><span className="font-display text-xs font-bold text-aurora-text-primary">{selectedBulkArtifacts.length === 1 ? '1 artifact selected' : `${selectedBulkArtifacts.length} artifacts selected`}</span><span className="min-w-2 flex-1"/>{compareEligible ? <Button variant="outline" size="icon-sm" className="size-7 rounded-lg" aria-label="Compare selected bundles" title="Compare selected bundles" onClick={()=>setCompareOpen(true)}><ArrowLeftRight className="size-3"/></Button> : null}<Button variant="ghost" size="sm" className="h-7 rounded-lg px-[11px] text-[11.5px]" onClick={clearBulkSelection}>Clear</Button><Button size="sm" className="h-7 gap-1.5 rounded-lg px-[13px] text-[11.5px] font-bold" disabled={importing} onClick={()=>void bulkAddToLibrary()}><Plus className="size-3"/>{`Add ${selectedBulkArtifacts.length} to Library`}</Button></div> : null}
        <h2 id="artifact-results-title" className="sr-only">{activeQuery ? `Results for “${activeQuery}”` : shelfMeta.title}</h2>
        {state.window.historyExpired?<p role="status" className="text-xs text-aurora-text-muted">Earlier results left the bounded local window. Refresh this search to revisit older history.</p>:null}
        {visible.leadingRows>0?<div aria-hidden="true" style={{height:Math.min(visible.leadingRows*8,320)}} />:null}
        {query.trim().length>0&&query.trim().length<3?<p className="rounded-aurora-2 border border-dashed border-aurora-border-subtle px-5 py-10 text-center text-sm text-aurora-text-muted">Enter at least 3 characters to search.</p>:<ArtifactResults artifacts={results} activeQuery={activeQuery} loading={state.loading} incomplete={incomplete} view={view} density={density} now={now} selectedKey={selectedId&&selectedArtifactProvider?artifactKey(selectedArtifactProvider,selectedId):undefined} artifactHref={artifactHref} onReset={resetDiscovery} selectionMode={selectionMode} selectedBulkKeys={bulkSelectedKeys} cursorIndex={cursorIndex} onToggleSelected={toggleBulkSelection} onEnterSelectionMode={enterSelectionMode} onAdd={addArtifactToLibrary} onFork={USE_MOCK_DATA?previewFork:undefined} onSend={USE_MOCK_DATA?previewSend:undefined} isInLibrary={isArtifactInLibrary} actionPending={importing}/>}
        {state.cursor?<div ref={loadMoreRef} className="flex min-h-12 items-center justify-center" role="status" aria-live="polite"><Button variant="outline" onClick={()=>void load(activeQuery,state.cursor)} disabled={state.loading}>{state.loading?<Loader2 className="size-4 animate-spin"/>:null}{state.loading?'Loading more artifacts…':'Load more'}</Button></div>:null}
      </section>
      </div>
      </div>
    </div></div>
    <DiscoverBundleCompare open={compareOpen} artifacts={selectedBulkArtifacts} onOpenChange={setCompareOpen}/>
    <ArtifactInspection artifact={detail} previewMode={USE_MOCK_DATA} specLabels={detail&&USE_MOCK_DATA?mockDepotSpecLabels(detail):undefined} metricLabels={detail&&USE_MOCK_DATA?mockDepotMetricLabels(detail):undefined} inLibrary={detail?isArtifactInLibrary(detail):false} importing={importing} onImport={addArtifactToLibrary} onFork={USE_MOCK_DATA?previewFork:undefined} onSend={USE_MOCK_DATA?previewSend:undefined} installFormats={USE_MOCK_DATA?['Claude plugin.json','gemini-extension.json','Agent Plugins','mcp.json','ARD entry','Loadout']:undefined} onInstallFormat={USE_MOCK_DATA?previewInstallFormat:undefined} loading={detailLoading} open={Boolean(selectedId&&selectedArtifactProvider)} focusKey={selectedId && selectedArtifactProvider ? artifactKey(selectedArtifactProvider, selectedId) : undefined} copied={copied} onOpenChange={open=>{if(!open)router.push(artifactHref(),{scroll:false})}} onCopy={copyValue} onExport={exportArtifact}/>
  </>
}

export function ArtifactResults({artifacts,activeQuery,loading,incomplete,view,density,now,selectedKey,artifactHref,onReset,selectionMode,selectedBulkKeys,cursorIndex,onToggleSelected,onEnterSelectionMode,onAdd,onFork,onSend,isInLibrary,actionPending}:{artifacts:FederatedArtifact[];activeQuery:string;loading:boolean;incomplete:boolean;view:DiscoveryLayout;density:DiscoveryDensity;now?:number;selectedKey?:string;artifactHref:(providerId?:string,id?:string)=>string;onReset:()=>void;selectionMode:boolean;selectedBulkKeys:string[];cursorIndex:number;onToggleSelected:(artifact:FederatedArtifact)=>void;onEnterSelectionMode:(artifact:FederatedArtifact)=>void;onAdd:(artifact:FederatedArtifact)=>void|Promise<void>;onFork?:((artifact:FederatedArtifact)=>void|Promise<void>);onSend?:((artifact:FederatedArtifact)=>void|Promise<void>);isInLibrary:(artifact:FederatedArtifact)=>boolean;actionPending:boolean}){
  if(loading&&!artifacts.length)return <div className="flex min-h-56 items-center justify-center rounded-aurora-2 border border-dashed border-aurora-border-subtle text-sm text-aurora-text-muted"><Loader2 className="mr-2 size-4 animate-spin"/>Searching catalog…</div>
  if(!artifacts.length)return <div className="mt-3 rounded-aurora-2 border-[1.5px] border-dashed border-aurora-border-strong/55 px-[22px] py-11 text-center"><div className="font-display text-base font-[760] leading-[22px] text-aurora-text-primary">{incomplete?'Search results are not complete yet.':'No artifacts match that filter.'}</div><div className="mt-1.5 text-[12.5px] leading-[15px] text-aurora-text-muted">{incomplete?'Retry once every source is available.':'Clear the kind and source filters, or publish the first one.'}</div>{!incomplete?<Button className="mt-3.5 h-[30px] rounded-lg px-[13px] py-0 text-xs font-[650] leading-normal" variant="outline" onClick={onReset}>Reset Filters</Button>:null}</div>
  return <div className={view==='cards'?'grid grid-cols-[repeat(auto-fill,minmax(min(100%,268px),1fr))] items-start gap-3':'overflow-hidden rounded-aurora-2 border border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))] bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-[var(--aurora-shadow-medium),inset_0_1px_0_rgba(255,255,255,0.04)]'}><div className={view==='list'?'min-w-0 overflow-x-hidden':'contents'}>{artifacts.map((artifact,index)=>{ const key=artifactKey(artifact.providerId,artifact.artifactId); return <ArtifactCard key={key} artifact={artifact} compact={view==='list'} density={density} now={now} selected={selectedKey===key} href={artifactHref(artifact.providerId,artifact.artifactId)} selectionMode={selectionMode} selectedForBulk={selectedBulkKeys.includes(key)} cursorActive={cursorIndex===index} matchLabel={artifactMatchLabel(artifact,activeQuery)} specLabels={USE_MOCK_DATA?mockDepotSpecLabels(artifact):undefined} metricLabels={USE_MOCK_DATA?mockDepotMetricLabels(artifact):undefined} onToggleSelected={()=>onToggleSelected(artifact)} onEnterSelectionMode={()=>onEnterSelectionMode(artifact)} onAdd={()=>onAdd(artifact)} onFork={onFork?()=>onFork(artifact):undefined} onSend={onSend?()=>onSend(artifact):undefined} inLibrary={isInLibrary(artifact)} actionPending={actionPending}/> })}</div></div>
}
