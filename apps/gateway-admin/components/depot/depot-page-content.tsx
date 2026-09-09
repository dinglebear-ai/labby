'use client'

import { useCallback, useEffect, useRef, useState } from 'react'
import { usePathname, useRouter, useSearchParams } from 'next/navigation'
import Link from 'next/link'
import { Compass, Loader2, Plus, Search, SlidersHorizontal } from 'lucide-react'
import { toast } from 'sonner'

import { AppHeader } from '@/components/app-header'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { getArtifact, listArtifacts, listProviderOptions, type DepotArtifact, type DepotProviderOption, type FederatedArtifact } from '@/lib/api/depot-client'
import { artifactKey } from '@/lib/depot/provider-model'
import { appendDiscoveryPage, createDiscoveryWindow, visibleArtifacts, type DiscoveryWindow } from './discovery-window'
import { RequestLanes } from './request-lanes'
import { controlPlaneAction } from '@/lib/api/artifact-control-client'
import { createDepotImportAttempt } from '@/lib/api/depot-import-client'
import { artifactKind, artifactTitle, selectDiscoveryResults, type DiscoverySort } from './discover-model'
import { DiscoverArtifactCard as ArtifactCard } from './discover-artifact-card'
import { DISCOVER_SOURCE_ORIGINS } from './discover-source-badge'
import { DiscoverHighlights, type DiscoverHighlightsData } from './discover-highlights'
import { DiscoverFeedTabs } from './discover-feed-tabs'
import { DiscoverSourceStrip } from './discover-source-strip'
import { discoverSourceSupport } from './discover-source-support'
import { DiscoverArtifactInspection as ArtifactInspection } from './discover-artifact-inspection'
import { DiscoverViewOptions, type DiscoveryDensity } from './discover-view-options'
import { membershipSourceForArtifact, useDiscoverMembership } from './use-discover-membership'
import { DiscoverSendDialog, type DiscoverSendTarget } from './discover-send-dialog'
import { DiscoverForkDialog, type DiscoverForkTarget } from './discover-fork-dialog'
import { getBrowserSessionEpoch } from '@/lib/auth/session-store'

type LoadState = { loading: boolean; error?: string; window: DiscoveryWindow; cursor?: string; total?: number; exact: boolean; coverage?: string; scopeEpoch?: string; highlights?: DiscoverHighlightsData }
type View = 'cards' | 'list'

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

export function DepotPageContent() {
  const router = useRouter(), pathname = usePathname(), searchParams = useSearchParams()
  const selectedId = searchParams.get('artifact') ?? undefined, selectedArtifactProvider = searchParams.get('artifactProvider') ?? undefined
  const selectedProvider = searchParams.get('provider') ?? 'all', initialQuery = searchParams.get('q')?.trim() ?? ''
  const selectedFeed = searchParams.get('feed') === 'new' ? 'new' : undefined
  const selectedOrigin = searchParams.get('origin') ?? undefined
  const [query,setQuery] = useState(initialQuery), [activeQuery,setActiveQuery] = useState(initialQuery)
  const [state,setState] = useState<LoadState>({ loading:true, window:createDiscoveryWindow(), exact:false })
  const [providers,setProviders] = useState<DepotProviderOption[]>([])
  const [detail,setDetail] = useState<FederatedArtifact|null>(null), [detailLoading,setDetailLoading] = useState(false)
  const [copied,setCopied] = useState<string>(), [view,setView] = useState<View>('cards')
  const [importingKey,setImportingKey] = useState<string>()
  const importInFlight = useRef(false)
  const importing = importingKey !== undefined
  const [membershipVersion, setMembershipVersion] = useState(0)
  const [sendTarget, setSendTarget] = useState<DiscoverSendTarget | null>(null)
  const [forkTarget, setForkTarget] = useState<DiscoverForkTarget | null>(null)
  const openFork = useCallback((artifact: FederatedArtifact) => setForkTarget({ title: artifactTitle(artifact), source: membershipSourceForArtifact(artifact) }), [])
  const openSend = useCallback((artifact: FederatedArtifact) => setSendTarget({ title: artifactTitle(artifact), kind: artifactKind(artifact), source: membershipSourceForArtifact(artifact) }), [])
  const [density, setDensity] = useState<DiscoveryDensity>('default')
  const [now, setNow] = useState<number>()
  const lanes = useRef(new RequestLanes()), inFlight = useRef<string | undefined>(undefined)
  const loadMoreRef = useRef<HTMLDivElement>(null)
  const paginationControllerRef = useRef<AbortController>(null)
  const providerRefreshVersion = useRef(0)
  const refreshProviders = useCallback(async (signal?: AbortSignal) => {
    const version = ++providerRefreshVersion.current
    try {
      const options = await listProviderOptions(signal)
      if (version === providerRefreshVersion.current && !signal?.aborted) setProviders(options)
    } catch {
      if (version === providerRefreshVersion.current && !signal?.aborted) {
        setProviders(current => current.map(provider => ({ ...provider, sourceOrigins: null })))
      }
    }
  }, [])

  useEffect(() => {
    const refresh = () => setNow(Date.now())
    refresh()
    const timer = window.setInterval(refresh, 60_000)
    return () => window.clearInterval(timer)
  }, [])

  const load = useCallback(async (searchQuery:string,cursor?:string,signal?:AbortSignal) => {
    const key = JSON.stringify([selectedProvider,selectedOrigin,selectedFeed,searchQuery,cursor??null])
    if(inFlight.current===key)return
    const generation = lanes.current.begin('list')
    inFlight.current=key
    setState(c=>({...c,loading:true,error:undefined,window:cursor?c.window:createDiscoveryWindow(),cursor:cursor?c.cursor:undefined,total:cursor?c.total:undefined}))
    try {
      const listing = await listArtifacts({provider:selectedProvider,sourceOrigin:selectedOrigin,query:searchQuery,limit:50,cursor,feed:selectedFeed},signal)
      if(!lanes.current.isCurrent('list',generation)||signal?.aborted)return
      setState(c=>({loading:false,window:appendDiscoveryPage(cursor?c.window:createDiscoveryWindow(),listing.items),cursor:listing.nextCursor??undefined,total:listing.knownTotal??undefined,exact:listing.totalIsExact,coverage:listing.state,scopeEpoch:listing.scopeEpoch,highlights:listing.highlights}))
    } catch(error) { if(lanes.current.isCurrent('list',generation)&&!signal?.aborted)setState(c=>({...c,loading:false,error:error instanceof Error?error.message:String(error)})) }
    finally {
      if(inFlight.current===key)inFlight.current=undefined
      if (!signal?.aborted) void refreshProviders(signal)
    }
  },[selectedProvider,selectedOrigin,selectedFeed,refreshProviders])

  useEffect(()=>{const controller=new AbortController();void refreshProviders(controller.signal);return()=>controller.abort()},[refreshProviders])
  useEffect(()=>()=>paginationControllerRef.current?.abort(),[])

  useEffect(()=>{ const controller=new AbortController(); const timer=window.setTimeout(()=>{ const next=query.trim(); setActiveQuery(next); const params=new URLSearchParams(window.location.search); if((params.get('q')?.trim()??'')!==next){if(next)params.set('q',next);else params.delete('q');params.delete('artifact');params.delete('artifactProvider');router.replace(`${pathname}${params.size?`?${params}`:''}`,{scroll:false})} if(next.length===0||next.length>=3)void load(next,undefined,controller.signal);else setState(current=>({...current,loading:false,error:undefined,window:createDiscoveryWindow(),cursor:undefined,total:undefined,exact:false}))},query?300:0); return()=>{window.clearTimeout(timer);controller.abort()} },[load,pathname,query,router])
  useEffect(()=>{const generation=lanes.current.begin('detail');if(!selectedId||!selectedArtifactProvider){setDetail(null);setDetailLoading(false);return}const controller=new AbortController();setDetail(null);setDetailLoading(true);void getArtifact(selectedArtifactProvider,selectedId,controller.signal).then(r=>{if(lanes.current.isCurrent('detail',generation))setDetail({...r.artifact,providerId:r.providerId,artifactId:r.artifactId})}).catch(e=>{if(lanes.current.isCurrent('detail',generation)&&!controller.signal.aborted)toast.error(e instanceof Error?e.message:String(e))}).finally(()=>{if(lanes.current.isCurrent('detail',generation)&&!controller.signal.aborted)setDetailLoading(false)});return()=>controller.abort()},[selectedArtifactProvider,selectedId])
  useEffect(()=>{const target=loadMoreRef.current;if(!target||!state.cursor||state.loading||state.error)return;const observer=new IntersectionObserver(entries=>{if(entries.some(entry=>entry.isIntersecting)){observer.disconnect();const controller=new AbortController();paginationControllerRef.current=controller;void load(activeQuery,state.cursor,controller.signal)}},{rootMargin:'600px 0px'});observer.observe(target);return()=>observer.disconnect()},[activeQuery,load,state.cursor,state.error,state.loading])

  const artifactHref=useCallback((providerId?:string,id?:string)=>{const params=new URLSearchParams();if(activeQuery)params.set('q',activeQuery);if(selectedProvider!=='all')params.set('provider',selectedProvider);if(selectedOrigin)params.set('origin',selectedOrigin);if(selectedFeed)params.set('feed',selectedFeed);if(providerId&&id){params.set('artifactProvider',providerId);params.set('artifact',id)}return `${pathname}${params.size?`?${params}`:''}`},[activeQuery,pathname,selectedProvider,selectedOrigin,selectedFeed])
  const onInspectionOpenChange = useCallback((open: boolean) => {
    if (open) return
    // Selection is same-page URL state. Next's history integration updates
    // search params without reusing a static route's hydration-time query.
    window.history.pushState(null, '', artifactHref())
  }, [artifactHref])
  const copyValue=useCallback(async(label:string,value?:string)=>{if(!value)return;await navigator.clipboard.writeText(value);setCopied(label);toast.success(`${label} copied`);window.setTimeout(()=>setCopied(c=>c===label?undefined:c),1500)},[])
  const exportArtifact=useCallback((artifact:FederatedArtifact)=>{const label=artifact.name??artifact.descriptor?.name??artifact.kind??'artifact';const blob=new Blob([`${JSON.stringify(artifact,null,2)}\n`],{type:'application/json'}),url=URL.createObjectURL(blob),anchor=document.createElement('a');anchor.href=url;anchor.download=`${label.toLowerCase().replace(/[^a-z0-9._-]+/g,'-')}.depot.json`;anchor.click();URL.revokeObjectURL(url);toast.success('Artifact metadata exported')},[])
  const importArtifact=useCallback(async(artifact:FederatedArtifact)=>{
    if(importInFlight.current)return
    importInFlight.current=true
    const importEpoch = getBrowserSessionEpoch()
    setImportingKey(artifactKey(artifact.providerId,artifact.artifactId))
    try{
      const artifactId=artifact.artifactId||artifact.id
      const revisionId=artifact.currentRevisionId||artifact.currentRevision?.id
      if(!artifactId||!revisionId)throw new Error('Depot did not provide an exact Artifact and revision identity.')
      const [connectionResult,libraryResult]=await Promise.all([
        controlPlaneAction<{connections?:Array<{id:string}>}>('artifacts','artifacts.list_connections'),
        controlPlaneAction<{library_version?:number}>('artifacts','artifacts.list',{limit:1}),
      ])
      const connectionId=exactImportConnection(artifact.providerId,connectionResult.connections??[])
      if(!Number.isSafeInteger(libraryResult.library_version)||Number(libraryResult.library_version)<0)throw new Error('Labby did not return a valid current library version.')
      if(importEpoch !== getBrowserSessionEpoch())throw new Error('Your session or project changed. Add the artifact again from the current library context.')
      await createDepotImportAttempt({ connection_id: connectionId, artifact_id: artifactId, revision_id: revisionId }, Number(libraryResult.library_version), importEpoch)()
      if(importEpoch !== getBrowserSessionEpoch())return
      toast.success('Exact Artifact revision added to your library')
      setMembershipVersion(version => version + 1)
    }catch(error){toast.error(error instanceof Error?error.message:String(error))}
    finally{importInFlight.current=false;setImportingKey(undefined)}
  },[])
  const visible = visibleArtifacts(state.window)
  const [kind, setKind] = useState('all')
  const [sort, setSort] = useState<DiscoverySort>('catalog')
  const kinds = [...new Set([...visible.items.map(artifactKind), ...(kind === 'all' ? [] : [kind])])].sort()
  const results = selectDiscoveryResults(visible.items, kind, selectedFeed === 'new' ? 'catalog' : sort)
  const isInLibrary = useDiscoverMembership(detail ? [...results, detail] : results, membershipVersion)
  const resultCount=state.total??state.window.rowCount
  const selectOrigin = (origin?: string) => {
    if (origin === selectedOrigin) return
    lanes.current.invalidate('list')
    paginationControllerRef.current?.abort()
    setState({ loading: true, window: createDiscoveryWindow(), exact: false })
    setKind('all')
    const params = new URLSearchParams(window.location.search)
    if (origin) params.set('origin', origin)
    else params.delete('origin')
    params.delete('artifact')
    params.delete('artifactProvider')
    // Discover is a client-owned query over a static export. Next observes native
    // history changes without fetching a new server-component route payload.
    window.history.replaceState(null, '', `${pathname}${params.size ? `?${params}` : ''}`)
  }
  const selectProvider = (providerId: string) => {
    if (providerId === selectedProvider) return
    lanes.current.invalidate('list')
    setKind('all')
    const params = new URLSearchParams(window.location.search)
    if (providerId === 'all') params.delete('provider')
    else params.set('provider', providerId)
    params.delete('artifact')
    params.delete('artifactProvider')
    router.replace(`${pathname}${params.size ? `?${params}` : ''}`, { scroll: false })
  }

  return <>
    <AppHeader icon={<Compass className="size-3.5" />} breadcrumbs={[{label:'Discover'}]}/>
    <div className={`${AURORA_PAGE_SHELL} flex-1`}><div className={`${AURORA_PAGE_FRAME} gap-[14px]`}>
      <ConsoleHero variant="discover" icon={<Compass className="size-[22px]" />} eyebrow="Depot · Bazaar" title="Discover" description="Every artifact Depot can reach — registries, marketplaces, catalogs and crawls — searched semantically and installable in any target format through APM." pulse={depotCoveragePulse(state.coverage,state.error)} actions={<Button data-visible-label asChild variant="outline" className="h-9 rounded-[10px] px-4 font-[650] text-aurora-accent-strong" style={{ borderColor: 'color-mix(in srgb, var(--aurora-accent-primary) 55%, var(--aurora-border-strong))', background: 'color-mix(in srgb, var(--aurora-accent-primary) 9%, var(--aurora-panel-strong))' }}><Link href="/create"><Plus aria-hidden="true" className="size-3.5" />Publish Artifact</Link></Button>}
        stats={[
          { label: activeQuery ? 'Matches' : selectedFeed === 'new' ? 'New in 7 days' : 'Indexed', value: discoveryCountLabel(resultCount, state.exact, Boolean(state.error) || (state.loading && !state.window.rowCount)), suffix: resultCount === 1 ? 'artifact' : 'artifacts' },
          { label: 'Sources', value: providers.filter(provider => provider.enabled).length, suffix: 'connected backends' },
          { label: 'Last crawl', value: <span title="Crawl timestamps are not reported by the connected sources." className="text-sm font-normal text-aurora-text-muted">Not reported</span> },
          { label: 'Verified publishers', value: <span title="Publisher verification is not reported by the connected sources." className="text-sm font-normal text-aurora-text-muted">Not reported</span> },
        ]}>
        <div className="space-y-[var(--space-3)] px-6 py-3.5 sm:pl-[82px]">
          <div className="flex min-w-0 items-center gap-[9px] rounded-[12px] border border-aurora-border-strong bg-aurora-control-surface px-[13px] focus-within:ring-2 focus-within:ring-aurora-accent-primary">
            <Search aria-hidden="true" className="size-4 shrink-0 text-aurora-accent-strong" />
            <Input name="artifact-search" aria-label="Search Depot artifacts" className="h-10 min-w-0 flex-1 border-0 bg-transparent px-0 font-medium shadow-none focus-visible:ring-0" value={query}
              onChange={event => { lanes.current.invalidate('list'); setQuery(event.target.value) }}
              placeholder="Search artifacts across your sources" />
            <DiscoverSourceStrip selectedOrigin={selectedOrigin} onSelect={selectOrigin} providers={providers} selectedProvider={selectedProvider} />
            <Popover>
              <PopoverTrigger asChild><Button variant="ghost" size="icon-sm" className="shrink-0" aria-label="Kind and source filters"><SlidersHorizontal aria-hidden="true" className="size-4" /></Button></PopoverTrigger>
              <PopoverContent align="end" aria-label="Kind and source filters" className="aurora-scrollbar max-h-[min(32rem,70svh)] w-80 max-w-[calc(100vw-2rem)] space-y-4 overflow-y-auto rounded-aurora-2 border-aurora-border-strong bg-aurora-panel-strong text-aurora-text-primary">
                <fieldset className="space-y-2"><legend className="text-sm font-semibold">Artifact kind</legend>
                  <div className="flex flex-wrap gap-2">{['all', ...kinds].map(value => <Button key={value} size="sm" variant={kind === value ? 'secondary' : 'ghost'} aria-pressed={kind === value} onClick={() => setKind(value)}>{value === 'all' ? 'All kinds' : value}</Button>)}</div>
                </fieldset>
                <fieldset className="space-y-2"><legend className="text-sm font-semibold">Source origin</legend>
                  <Button size="sm" variant={!selectedOrigin ? 'secondary' : 'ghost'} aria-pressed={!selectedOrigin} onClick={() => selectOrigin()}>All origins</Button>
                  {Object.entries(DISCOVER_SOURCE_ORIGINS).map(([origin, source]) => {
                    const support = discoverSourceSupport(providers, selectedProvider, origin as keyof typeof DISCOVER_SOURCE_ORIGINS)
                    return <Button data-visible-label key={origin} size="sm" variant={selectedOrigin === origin ? 'secondary' : 'ghost'} aria-pressed={selectedOrigin === origin} aria-disabled={support.state !== 'available' || undefined} title={support.reason} onClick={() => { if (support.state === 'available') selectOrigin(origin) }}><source.icon aria-hidden="true" className="size-4" style={{ color: source.color }} />{source.label}</Button>
                  })}
                </fieldset>
                <fieldset className="space-y-2"><legend className="text-sm font-semibold">Depot backend</legend>
                  {[{ id: 'all', name: 'All sources', enabled: true }, ...providers].map(provider => <Button key={provider.id} size="sm" className="w-full justify-start" variant={selectedProvider === provider.id ? 'secondary' : 'ghost'} disabled={!provider.enabled} aria-pressed={selectedProvider === provider.id} onClick={() => selectProvider(provider.id)}>{provider.name}</Button>)}
                </fieldset>
                <p className="text-xs text-aurora-text-muted">Kinds filter the retained results. Sources change the catalog search.</p>
              </PopoverContent>
            </Popover>
          </div>
          {selectedProvider !== 'all' ? <p className="text-xs text-aurora-text-muted">Source: {providers.find(provider => provider.id === selectedProvider)?.name ?? selectedProvider}</p> : null}
          {selectedOrigin ? <p className="text-xs text-aurora-text-muted">Origin: {DISCOVER_SOURCE_ORIGINS[selectedOrigin as keyof typeof DISCOVER_SOURCE_ORIGINS]?.label ?? selectedOrigin}</p> : null}
        </div>
      </ConsoleHero>
      {!activeQuery && !selectedOrigin && selectedProvider === 'all' && !selectedFeed && !state.error ? <DiscoverHighlights feeds={state.loading ? { popular: { state: 'loading' }, team: { state: 'loading' }, loadouts: { state: 'loading' } } : state.highlights} artifactHref={artifactHref} /> : null}
      {state.error?<DashboardPanel title="Depot unavailable"><p role="alert" className="text-sm text-aurora-error">{state.error}. Labby-only routes remain available.</p></DashboardPanel>:null}
      <section aria-labelledby="artifact-results-title" className="@container/discover-results space-y-3">
        <div className="flex flex-wrap items-end gap-2.5 border-b border-aurora-border-subtle px-0.5">
          <DiscoverFeedTabs selected={selectedFeed} onSelect={feed => {
                if (selectedFeed === feed) return
                lanes.current.invalidate('list')
                paginationControllerRef.current?.abort()
                setState({ loading: true, window: createDiscoveryWindow(), exact: false })
                setSort('catalog')
                const params = new URLSearchParams(window.location.search)
                if (feed) params.set('feed', feed)
                else params.delete('feed')
                params.delete('artifact')
                params.delete('artifactProvider')
                router.replace(`${pathname}${params.size ? `?${params}` : ''}`, { scroll: false })
              }} />
          <div className="mb-1 flex flex-1 items-center justify-end gap-2.5">
            <span title={`${state.window.rowCount} results retained in the bounded local window`} className="inline-flex h-[22px] shrink-0 items-center rounded-[6px] border border-aurora-border-subtle bg-aurora-control-surface px-[9px] text-[10.5px] font-[650] tabular-nums text-aurora-text-muted">{state.loading ? 'Searching…' : `${results.length} of ${resultCount} shown`}</span>
            <DiscoverViewOptions ranked={selectedFeed === 'new'} sort={sort} setSort={setSort} layout={view} setLayout={setView} density={density} setDensity={setDensity} />
          </div>
        </div>
        {selectedFeed === 'new' ? <p className="text-xs text-aurora-text-muted">First observed in the past seven days, newest first. Older artifacts with unknown first-seen dates are excluded.</p> : null}
        <div className="flex flex-wrap items-baseline gap-2 px-0.5"><h2 id="artifact-results-title" className="font-display text-base font-extrabold text-aurora-text-primary">{activeQuery?`Results for “${activeQuery}”`:selectedFeed === 'new' ? 'Newly Published' : 'Catalog results'}</h2>{kind !== 'all' ? <Button size="sm" variant="ghost" onClick={() => setKind('all')}>{kind} · Clear kind filter</Button> : null}</div>
        {state.window.historyExpired?<p role="status" className="text-xs text-aurora-text-muted">Earlier results left the bounded local window. Refresh this search to revisit older history.</p>:null}
        {visible.leadingRows>0?<div aria-hidden="true" style={{height:Math.min(visible.leadingRows*8,320)}} />:null}
        {query.length>0&&query.length<3?<p className="text-sm text-aurora-text-muted">Enter at least 3 characters to search.</p>:<ArtifactResults artifacts={results} loading={state.loading} view={view} density={density} now={now} importingKey={importingKey} onImport={importArtifact} onSend={openSend} onFork={openFork} isInLibrary={isInLibrary} selectedKey={selectedId&&selectedArtifactProvider?artifactKey(selectedArtifactProvider,selectedId):undefined} artifactHref={artifactHref}/>}
        {state.cursor?<div ref={loadMoreRef} className="flex min-h-12 items-center justify-center" role="status" aria-live="polite"><Button variant="outline" onClick={()=>void load(activeQuery,state.cursor)} disabled={state.loading}>{state.loading?<Loader2 className="size-4 animate-spin"/>:null}{state.loading?'Loading more artifacts…':'Load more'}</Button></div>:null}
      </section>
    </div></div>
    <ArtifactInspection artifact={detail} importing={importing} inLibrary={detail ? isInLibrary(detail) : false} onImport={importArtifact} onSend={openSend} onFork={openFork} loading={detailLoading} open={Boolean(selectedId&&selectedArtifactProvider) && !sendTarget && !forkTarget} focusKey={selectedId && selectedArtifactProvider ? artifactKey(selectedArtifactProvider, selectedId) : undefined} copied={copied} onOpenChange={onInspectionOpenChange} onCopy={copyValue} onExport={exportArtifact}/>
    <DiscoverSendDialog target={sendTarget} onOpenChange={open => { if (!open) setSendTarget(null) }} onActivated={() => setMembershipVersion(version => version + 1)} />
    <DiscoverForkDialog target={forkTarget} onOpenChange={open => { if (!open) setForkTarget(null) }} />
  </>
}

function ArtifactResults({artifacts,loading,view,density,now,importingKey,onImport,onSend,onFork,isInLibrary,selectedKey,artifactHref}:{artifacts:FederatedArtifact[];loading:boolean;view:View;density:DiscoveryDensity;now?:number;importingKey?:string;onImport:(artifact:FederatedArtifact)=>Promise<void>;onSend:(artifact:FederatedArtifact)=>void;onFork:(artifact:FederatedArtifact)=>void;isInLibrary:(artifact:FederatedArtifact)=>boolean;selectedKey?:string;artifactHref:(providerId?:string,id?:string)=>string}){
  if(loading&&!artifacts.length)return <div className="flex min-h-56 items-center justify-center rounded-aurora-2 border border-dashed border-aurora-border-subtle text-sm text-aurora-text-muted"><Loader2 className="mr-2 size-4 animate-spin"/>Searching Bazaar…</div>
  if(!artifacts.length)return <div className="flex min-h-56 items-center justify-center rounded-aurora-2 border border-dashed border-aurora-border-subtle text-sm text-aurora-text-muted">No artifacts match this search.</div>
  return <div className={view==='cards'?'grid grid-cols-[repeat(auto-fill,minmax(min(100%,268px),1fr))] items-start gap-3':'space-y-2'}>{artifacts.map(artifact=><ArtifactCard key={artifactKey(artifact.providerId,artifact.artifactId)} artifact={artifact} compact={view==='list'} density={density} now={now} importing={importingKey===artifactKey(artifact.providerId,artifact.artifactId)} importDisabled={importingKey!==undefined} inLibrary={isInLibrary(artifact)} onImport={onImport} onSend={onSend} onFork={onFork} selected={selectedKey===artifactKey(artifact.providerId,artifact.artifactId)} href={artifactHref(artifact.providerId,artifact.artifactId)}/>)}</div>
}
