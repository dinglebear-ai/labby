'use client'

import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { usePathname, useRouter, useSearchParams } from 'next/navigation'
import Link from 'next/link'
import { Loader2, Search, SlidersHorizontal } from 'lucide-react'
import { toast } from 'sonner'

import { AppHeader } from '@/components/app-header'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { getArtifact, listArtifacts, listProviderOptions, type DepotArtifact, type DepotProviderOption, type FederatedArtifact } from '@/lib/api/depot-client'
import { getBrowserSessionEpoch, subscribeToBrowserSession } from '@/lib/auth/session-store'
import { artifactKey } from '@/lib/depot/provider-model'
import { appendDiscoveryPage, createDiscoveryWindow, visibleArtifacts, type DiscoveryWindow } from './discovery-window'
import { RequestLanes } from './request-lanes'
import { controlPlaneAction } from '@/lib/api/artifact-control-client'
import { artifactKind, selectDiscoveryResults, type DiscoverySort } from './discover-model'
import { DiscoverArtifactCard as ArtifactCard } from './discover-artifact-card'
import { DiscoverArtifactInspection as ArtifactInspection } from './discover-artifact-inspection'
import { DiscoverViewOptions, type DiscoveryDensity } from './discover-view-options'

type LoadState = { loading: boolean; error?: string; window: DiscoveryWindow; cursor?: string; total?: number; exact: boolean; coverage?: string; scopeEpoch?: string }
type View = 'cards' | 'list'

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
  const sessionEpoch = useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionEpoch, () => 0)
  return <SessionDepotPage key={sessionEpoch} />
}

function SessionDepotPage() {
  const router = useRouter(), pathname = usePathname(), searchParams = useSearchParams()
  const selectedId = searchParams.get('artifact') ?? undefined, selectedArtifactProvider = searchParams.get('artifactProvider') ?? undefined
  const selectedProvider = searchParams.get('provider') ?? 'all', initialQuery = searchParams.get('q')?.trim() ?? ''
  const [query,setQuery] = useState(initialQuery), [activeQuery,setActiveQuery] = useState(initialQuery)
  const [state,setState] = useState<LoadState>({ loading:true, window:createDiscoveryWindow(), exact:false })
  const [providers,setProviders] = useState<DepotProviderOption[]>([])
  const [detail,setDetail] = useState<FederatedArtifact|null>(null), [detailLoading,setDetailLoading] = useState(false)
  const [copied,setCopied] = useState<string>(), [view,setView] = useState<View>('cards')
  const [importing,setImporting] = useState(false)
  const importPending = useRef(false)
  const [density, setDensity] = useState<DiscoveryDensity>('default')
  const [now, setNow] = useState<number>()
  const lanes = useRef(new RequestLanes()), inFlight = useRef<string | undefined>(undefined)
  const loadMoreRef = useRef<HTMLDivElement>(null)
  const paginationControllerRef = useRef<AbortController>(null)

  useEffect(() => {
    const refresh = () => setNow(Date.now())
    refresh()
    const timer = window.setInterval(refresh, 60_000)
    return () => window.clearInterval(timer)
  }, [])

  const load = useCallback(async (searchQuery:string,cursor?:string,signal?:AbortSignal) => {
    const key = JSON.stringify([selectedProvider,searchQuery,cursor??null])
    if(inFlight.current===key)return
    const generation = lanes.current.begin('list')
    const epoch = getBrowserSessionEpoch()
    const isCurrent = () => epoch === getBrowserSessionEpoch() && lanes.current.isCurrent('list', generation) && !signal?.aborted
    inFlight.current=key
    setState(c=>({...c,loading:true,error:undefined,window:cursor?c.window:createDiscoveryWindow(),cursor:cursor?c.cursor:undefined,total:cursor?c.total:undefined}))
    try {
      const listing = await listArtifacts({provider:selectedProvider,query:searchQuery,limit:50,cursor},signal)
      if(!isCurrent())return
      setState(c=>({loading:false,window:appendDiscoveryPage(cursor?c.window:createDiscoveryWindow(),listing.items),cursor:listing.nextCursor??undefined,total:listing.knownTotal??undefined,exact:listing.totalIsExact,coverage:listing.state,scopeEpoch:listing.scopeEpoch}))
    } catch(error) { if(isCurrent())setState(c=>({...c,loading:false,error:error instanceof Error?error.message:String(error)})) }
    finally { if(inFlight.current===key)inFlight.current=undefined }
  },[selectedProvider])

  useEffect(()=>{const controller=new AbortController();const epoch=getBrowserSessionEpoch();void listProviderOptions(controller.signal).then(value=>{if(!controller.signal.aborted&&epoch===getBrowserSessionEpoch())setProviders(value)}).catch(()=>{});return()=>controller.abort()},[])
  useEffect(()=>()=>paginationControllerRef.current?.abort(),[])

  useEffect(()=>{ const controller=new AbortController(); const timer=window.setTimeout(()=>{ const next=query.trim(); setActiveQuery(next); const params=new URLSearchParams(window.location.search); if((params.get('q')?.trim()??'')!==next){if(next)params.set('q',next);else params.delete('q');params.delete('artifact');params.delete('artifactProvider');router.replace(`${pathname}${params.size?`?${params}`:''}`,{scroll:false})} if(next.length===0||next.length>=3)void load(next,undefined,controller.signal);else setState(current=>({...current,loading:false,error:undefined,window:createDiscoveryWindow(),cursor:undefined,total:undefined,exact:false}))},query?300:0); return()=>{window.clearTimeout(timer);controller.abort()} },[load,pathname,query,router])
  useEffect(()=>{const generation=lanes.current.begin('detail');const epoch=getBrowserSessionEpoch();if(!selectedId||!selectedArtifactProvider){setDetail(null);setDetailLoading(false);return}const controller=new AbortController();setDetail(null);setDetailLoading(true);void getArtifact(selectedArtifactProvider,selectedId,controller.signal).then(r=>{if(lanes.current.isCurrent('detail',generation)&&!controller.signal.aborted&&epoch===getBrowserSessionEpoch())setDetail({...r.artifact,providerId:r.providerId,artifactId:r.artifactId})}).catch(e=>{if(lanes.current.isCurrent('detail',generation)&&!controller.signal.aborted&&epoch===getBrowserSessionEpoch())toast.error(e instanceof Error?e.message:String(e))}).finally(()=>{if(lanes.current.isCurrent('detail',generation)&&!controller.signal.aborted&&epoch===getBrowserSessionEpoch())setDetailLoading(false)});return()=>controller.abort()},[selectedArtifactProvider,selectedId])
  useEffect(()=>{const target=loadMoreRef.current;if(!target||!state.cursor||state.loading||state.error)return;const observer=new IntersectionObserver(entries=>{if(entries.some(entry=>entry.isIntersecting)){observer.disconnect();const controller=new AbortController();paginationControllerRef.current=controller;void load(activeQuery,state.cursor,controller.signal)}},{rootMargin:'600px 0px'});observer.observe(target);return()=>observer.disconnect()},[activeQuery,load,state.cursor,state.error,state.loading])

  const artifactHref=useCallback((providerId?:string,id?:string)=>{const params=new URLSearchParams();if(activeQuery)params.set('q',activeQuery);if(selectedProvider!=='all')params.set('provider',selectedProvider);if(providerId&&id){params.set('artifactProvider',providerId);params.set('artifact',id)}return `${pathname}${params.size?`?${params}`:''}`},[activeQuery,pathname,selectedProvider])
  const copyValue=useCallback(async(label:string,value?:string)=>{if(!value)return;await navigator.clipboard.writeText(value);setCopied(label);toast.success(`${label} copied`);window.setTimeout(()=>setCopied(c=>c===label?undefined:c),1500)},[])
  const exportArtifact=useCallback((artifact:FederatedArtifact)=>{const label=artifact.name??artifact.descriptor?.name??artifact.kind??'artifact';const blob=new Blob([`${JSON.stringify(artifact,null,2)}\n`],{type:'application/json'}),url=URL.createObjectURL(blob),anchor=document.createElement('a');anchor.href=url;anchor.download=`${label.toLowerCase().replace(/[^a-z0-9._-]+/g,'-')}.depot.json`;anchor.click();URL.revokeObjectURL(url);toast.success('Artifact metadata exported')},[])
  const importArtifact=useCallback(async(artifact:FederatedArtifact)=>{
    if(importPending.current)return
    const epoch=getBrowserSessionEpoch()
    const artifactId=artifact.artifactId||artifact.id
    const revisionId=artifact.currentRevisionId||artifact.currentRevision?.id
    if(!artifactId||!revisionId)throw new Error('Depot did not provide an exact Artifact and revision identity.')
    importPending.current=true
    setImporting(true)
    try{
      const [connectionResult,libraryResult]=await Promise.all([
        controlPlaneAction<{connections?:Array<{id:string}>}>('artifacts','artifacts.list_connections'),
        controlPlaneAction<{library_version?:number}>('artifacts','artifacts.list',{limit:1}),
      ])
      if(epoch!==getBrowserSessionEpoch())return
      const connectionId=exactImportConnection(artifact.providerId,connectionResult.connections??[])
      if(!Number.isSafeInteger(libraryResult.library_version)||Number(libraryResult.library_version)<0)throw new Error('Labby did not return a valid current library version.')
      await controlPlaneAction('artifacts','artifacts.import',{
        source:{kind:'depot',connection_id:connectionId,artifact_id:artifactId,revision_id:revisionId},
        expected_library_version:libraryResult.library_version,
        idempotency_key:`depot-import-${crypto.randomUUID()}`,
      })
      if(epoch===getBrowserSessionEpoch())toast.success('Exact Artifact imported into Labby')
    }catch(error){if(epoch===getBrowserSessionEpoch())toast.error(error instanceof Error?error.message:String(error))}
    finally{importPending.current=false;setImporting(false)}
  },[])
  const visible = visibleArtifacts(state.window)
  const [kind, setKind] = useState('all')
  const [sort, setSort] = useState<DiscoverySort>('catalog')
  const kinds = [...new Set([...visible.items.map(artifactKind), ...(kind === 'all' ? [] : [kind])])].sort()
  const results = selectDiscoveryResults(visible.items, kind, sort)
  const resultCount=state.total??state.window.rowCount

  return <>
    <AppHeader breadcrumbs={[{label:'Depot'},{label:'Discover'}]}/>
    <div className={`${AURORA_PAGE_SHELL} flex-1`}><div className={AURORA_PAGE_FRAME}>
      <ConsoleHero eyebrow="Depot · Bazaar" title="Discover" description="Explore artifacts across your connected registries, marketplaces, catalogs, and crawls." pulse={depotCoveragePulse(state.coverage,state.error)} actions={<Button asChild variant="outline"><Link href="/create">Publish Artifact</Link></Button>}
        stats={[
          { label: activeQuery ? 'Matches' : 'Indexed', value: state.loading && !state.window.rowCount ? '—' : `${state.exact ? '' : '≥ '}${resultCount.toLocaleString()}` },
          { label: 'Sources', value: providers.filter(provider => provider.enabled).length },
          { label: 'Last crawl', value: <span title="Crawl timestamps are not reported by the connected sources." className="text-sm font-normal text-aurora-text-muted">Not reported</span> },
          { label: 'Verified publishers', value: <span title="Publisher verification is not reported by the connected sources." className="text-sm font-normal text-aurora-text-muted">Not reported</span> },
        ]}>
        <div className="space-y-[var(--space-5)]">
          <div className="relative mt-5">
            <Search aria-hidden="true" className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-accent-primary" />
            <Input name="artifact-search" aria-label="Search Depot artifacts" className="h-11 pl-10 pr-14" value={query}
              onChange={event => { lanes.current.invalidate('list'); setQuery(event.target.value) }}
              placeholder="Search artifacts across your sources" />
            <Popover>
              <PopoverTrigger asChild><Button variant="ghost" size="icon-sm" className="absolute right-1 top-1/2 -translate-y-1/2" aria-label="Kind and source filters"><SlidersHorizontal aria-hidden="true" className="size-4" /></Button></PopoverTrigger>
              <PopoverContent align="end" aria-label="Kind and source filters" className="aurora-scrollbar max-h-[min(32rem,70svh)] w-80 max-w-[calc(100vw-2rem)] space-y-4 overflow-y-auto rounded-aurora-2 border-aurora-border-strong bg-aurora-panel-strong text-aurora-text-primary">
                <fieldset className="space-y-2"><legend className="text-sm font-semibold">Artifact kind</legend>
                  <div className="flex flex-wrap gap-2">{['all', ...kinds].map(value => <Button key={value} size="sm" variant={kind === value ? 'secondary' : 'ghost'} aria-pressed={kind === value} onClick={() => setKind(value)}>{value === 'all' ? 'All kinds' : value}</Button>)}</div>
                </fieldset>
                <fieldset className="space-y-2"><legend className="text-sm font-semibold">Source</legend>
                  {[{ id: 'all', name: 'All sources', enabled: true }, ...providers].map(provider => <Button key={provider.id} size="sm" className="w-full justify-start" variant={selectedProvider === provider.id ? 'secondary' : 'ghost'} disabled={!provider.enabled} aria-pressed={selectedProvider === provider.id} onClick={() => {
                    if (provider.id === selectedProvider) return
                    lanes.current.invalidate('list')
                    setKind('all')
                    const params = new URLSearchParams(window.location.search)
                    if (provider.id === 'all') params.delete('provider')
                    else params.set('provider', provider.id)
                    params.delete('artifact')
                    params.delete('artifactProvider')
                    router.replace(`${pathname}${params.size ? `?${params}` : ''}`, { scroll: false })
                  }}>{provider.name}</Button>)}
                </fieldset>
                <p className="text-xs text-aurora-text-muted">Kinds filter the retained results. Sources change the catalog search.</p>
              </PopoverContent>
            </Popover>
          </div>
          <div className="flex flex-wrap items-center justify-between gap-[var(--space-3)]">
            <div role="group" aria-label="Depot sources" className="flex flex-wrap gap-[var(--space-2)]">
              {[{id: 'all', name: 'All sources', enabled: true}, ...providers].map(provider =>
                <Button key={provider.id} size="sm" variant={selectedProvider === provider.id ? 'secondary' : 'ghost'}
                  disabled={!provider.enabled} aria-pressed={selectedProvider === provider.id}
                  onClick={() => {
                    if (provider.id === selectedProvider) return
                    lanes.current.invalidate('list')
                    setKind('all')
                    const params = new URLSearchParams(window.location.search)
                    if (provider.id === 'all') params.delete('provider')
                    else params.set('provider', provider.id)
                    params.delete('artifact')
                    params.delete('artifactProvider')
                    router.replace(`${pathname}${params.size ? `?${params}` : ''}`, {scroll: false})
                  }}>{provider.name}</Button>
              )}
            </div>
          </div>
        </div>
      </ConsoleHero>
      {state.error?<DashboardPanel title="Depot unavailable"><p role="alert" className="text-sm text-aurora-error">{state.error}. Labby-only routes remain available.</p></DashboardPanel>:null}
      <section aria-labelledby="artifact-results-title" className="space-y-3">
        <div className="flex flex-wrap items-center gap-3 border-b border-aurora-border-default pb-3">
          <div role="group" aria-label="Artifact kind" className="flex flex-1 flex-wrap gap-2">
            {['all', ...kinds].map(value => <Button key={value} size="sm" variant={kind === value ? 'secondary' : 'ghost'} aria-pressed={kind === value} onClick={() => setKind(value)}>{value === 'all' ? 'All artifacts' : value}</Button>)}
          </div>
          <DiscoverViewOptions sort={sort} setSort={setSort} layout={view} setLayout={setView} density={density} setDensity={setDensity} />
        </div>
        <div className="flex items-end justify-between gap-3 px-0.5"><h2 id="artifact-results-title" className="text-base font-semibold text-aurora-text-primary">{activeQuery?`Results for “${activeQuery}”`:'Catalog results'}</h2><span className="text-[11px] font-semibold text-aurora-text-muted">{state.loading?'Searching…':`${results.length} shown · ${state.window.rowCount} retained of ${resultCount}`}</span></div>
        {state.window.historyExpired?<p role="status" className="text-xs text-aurora-text-muted">Earlier results left the bounded local window. Refresh this search to revisit older history.</p>:null}
        {visible.leadingRows>0?<div aria-hidden="true" style={{height:Math.min(visible.leadingRows*8,320)}} />:null}
        {query.length>0&&query.length<3?<p className="text-sm text-aurora-text-muted">Enter at least 3 characters to search.</p>:<ArtifactResults artifacts={results} loading={state.loading} view={view} density={density} now={now} selectedKey={selectedId&&selectedArtifactProvider?artifactKey(selectedArtifactProvider,selectedId):undefined} artifactHref={artifactHref}/>}
        {state.cursor?<div ref={loadMoreRef} className="flex min-h-12 items-center justify-center" role="status" aria-live="polite"><Button variant="outline" onClick={()=>void load(activeQuery,state.cursor)} disabled={state.loading}>{state.loading?<Loader2 className="size-4 animate-spin"/>:null}{state.loading?'Loading more artifacts…':'Load more'}</Button></div>:null}
      </section>
    </div></div>
    <ArtifactInspection artifact={detail} importing={importing} onImport={importArtifact} loading={detailLoading} open={Boolean(selectedId&&selectedArtifactProvider)} focusKey={selectedId && selectedArtifactProvider ? artifactKey(selectedArtifactProvider, selectedId) : undefined} copied={copied} onOpenChange={open=>{if(!open)router.push(artifactHref(),{scroll:false})}} onCopy={copyValue} onExport={exportArtifact}/>
  </>
}

function ArtifactResults({artifacts,loading,view,density,now,selectedKey,artifactHref}:{artifacts:FederatedArtifact[];loading:boolean;view:View;density:DiscoveryDensity;now?:number;selectedKey?:string;artifactHref:(providerId?:string,id?:string)=>string}){
  if(loading&&!artifacts.length)return <div className="flex min-h-56 items-center justify-center rounded-aurora-2 border border-dashed border-aurora-border-subtle text-sm text-aurora-text-muted"><Loader2 className="mr-2 size-4 animate-spin"/>Searching Bazaar…</div>
  if(!artifacts.length)return <div className="flex min-h-56 items-center justify-center rounded-aurora-2 border border-dashed border-aurora-border-subtle text-sm text-aurora-text-muted">No artifacts match this search.</div>
  return <div className={view==='cards'?'grid gap-3 md:grid-cols-2 xl:grid-cols-3':'space-y-2'}>{artifacts.map(artifact=><ArtifactCard key={artifactKey(artifact.providerId,artifact.artifactId)} artifact={artifact} compact={view==='list'} density={density} now={now} selected={selectedKey===artifactKey(artifact.providerId,artifact.artifactId)} href={artifactHref(artifact.providerId,artifact.artifactId)}/>)}</div>
}
