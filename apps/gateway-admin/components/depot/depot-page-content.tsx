'use client'

import { useCallback, useEffect, useRef, useState } from 'react'
import { usePathname, useRouter, useSearchParams } from 'next/navigation'
import { Check, Copy, Download, Grid2X2, Layers3, Link2, List, Loader2, Lock, Search, X } from 'lucide-react'
import { toast } from 'sonner'

import { AppHeader } from '@/components/app-header'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { getArtifact, listArtifacts, listProviderOptions, type DepotArtifact, type DepotProviderOption, type FederatedArtifact } from '@/lib/api/depot-client'
import { artifactKey } from '@/lib/depot/provider-model'
import { appendDiscoveryPage, createDiscoveryWindow, visibleArtifacts, type DiscoveryWindow } from './discovery-window'
import { RequestLanes } from './request-lanes'

type LoadState = { loading: boolean; error?: string; window: DiscoveryWindow; cursor?: string; total?: number; exact: boolean; coverage?: string; scopeEpoch?: string }
type View = 'cards' | 'list'

export function mergeArtifactPages(current: DepotArtifact[], incoming: DepotArtifact[]): DepotArtifact[] {
  const seen = new Set(current.map(artifact => artifact.id ?? artifact.descriptor?.id).filter(Boolean))
  return [...current, ...incoming.filter(artifact => {
    const id = artifact.id ?? artifact.descriptor?.id
    if (!id || seen.has(id)) return false
    seen.add(id)
    return true
  })]
}

export function DepotPageContent() {
  const router = useRouter(), pathname = usePathname(), searchParams = useSearchParams()
  const selectedId = searchParams.get('artifact') ?? undefined, selectedArtifactProvider = searchParams.get('artifactProvider') ?? undefined
  const selectedProvider = searchParams.get('provider') ?? 'all', initialQuery = searchParams.get('q')?.trim() ?? ''
  const [query,setQuery] = useState(initialQuery), [activeQuery,setActiveQuery] = useState(initialQuery)
  const [state,setState] = useState<LoadState>({ loading:true, window:createDiscoveryWindow(), exact:false })
  const [providers,setProviders] = useState<DepotProviderOption[]>([])
  const [detail,setDetail] = useState<FederatedArtifact|null>(null), [detailLoading,setDetailLoading] = useState(false)
  const [copied,setCopied] = useState<string>(), [view,setView] = useState<View>('cards')
  const lanes = useRef(new RequestLanes()), inFlight = useRef<string | undefined>(undefined)
  const loadMoreRef = useRef<HTMLDivElement>(null)
  const paginationControllerRef = useRef<AbortController>(null)

  const load = useCallback(async (searchQuery:string,cursor?:string,signal?:AbortSignal) => {
    const key = JSON.stringify([selectedProvider,searchQuery,cursor??null])
    if(inFlight.current===key)return
    const generation = lanes.current.begin('list')
    inFlight.current=key
    setState(c=>({...c,loading:true,error:undefined,window:cursor?c.window:createDiscoveryWindow(),cursor:cursor?c.cursor:undefined,total:cursor?c.total:undefined}))
    try {
      const listing = await listArtifacts({provider:selectedProvider,query:searchQuery,limit:50,cursor},signal)
      if(!lanes.current.isCurrent('list',generation)||signal?.aborted)return
      setState(c=>({loading:false,window:appendDiscoveryPage(cursor?c.window:createDiscoveryWindow(),listing.items),cursor:listing.nextCursor??undefined,total:listing.knownTotal??undefined,exact:listing.totalIsExact,coverage:listing.state,scopeEpoch:listing.scopeEpoch}))
    } catch(error) { if(lanes.current.isCurrent('list',generation)&&!signal?.aborted)setState(c=>({...c,loading:false,error:error instanceof Error?error.message:String(error)})) }
    finally { if(inFlight.current===key)inFlight.current=undefined }
  },[selectedProvider])

  useEffect(()=>{const controller=new AbortController();void listProviderOptions(controller.signal).then(setProviders).catch(()=>{});return()=>controller.abort()},[])
  useEffect(()=>()=>paginationControllerRef.current?.abort(),[])

  useEffect(()=>{ const controller=new AbortController(); const timer=window.setTimeout(()=>{ const next=query.trim(); setActiveQuery(next); const params=new URLSearchParams(window.location.search); if((params.get('q')?.trim()??'')!==next){if(next)params.set('q',next);else params.delete('q');params.delete('artifact');params.delete('artifactProvider');router.replace(`${pathname}${params.size?`?${params}`:''}`,{scroll:false})} if(next.length===0||next.length>=3)void load(next,undefined,controller.signal);else setState(current=>({...current,loading:false,error:undefined,window:createDiscoveryWindow(),cursor:undefined,total:undefined,exact:false}))},query?300:0); return()=>{window.clearTimeout(timer);controller.abort()} },[load,pathname,query,router])
  useEffect(()=>{const generation=lanes.current.begin('detail');if(!selectedId||!selectedArtifactProvider){setDetail(null);setDetailLoading(false);return}const controller=new AbortController();setDetail(null);setDetailLoading(true);void getArtifact(selectedArtifactProvider,selectedId,controller.signal).then(r=>{if(lanes.current.isCurrent('detail',generation))setDetail({...r.artifact,providerId:r.providerId,artifactId:r.artifactId})}).catch(e=>{if(lanes.current.isCurrent('detail',generation)&&!controller.signal.aborted)toast.error(e instanceof Error?e.message:String(e))}).finally(()=>{if(lanes.current.isCurrent('detail',generation)&&!controller.signal.aborted)setDetailLoading(false)});return()=>controller.abort()},[selectedArtifactProvider,selectedId])
  useEffect(()=>{const target=loadMoreRef.current;if(!target||!state.cursor||state.loading||state.error)return;const observer=new IntersectionObserver(entries=>{if(entries.some(entry=>entry.isIntersecting)){observer.disconnect();const controller=new AbortController();paginationControllerRef.current=controller;void load(activeQuery,state.cursor,controller.signal)}},{rootMargin:'600px 0px'});observer.observe(target);return()=>observer.disconnect()},[activeQuery,load,state.cursor,state.error,state.loading])

  const artifactHref=useCallback((providerId?:string,id?:string)=>{const params=new URLSearchParams();if(activeQuery)params.set('q',activeQuery);if(selectedProvider!=='all')params.set('provider',selectedProvider);if(providerId&&id){params.set('artifactProvider',providerId);params.set('artifact',id)}return `${pathname}${params.size?`?${params}`:''}`},[activeQuery,pathname,selectedProvider])
  const copyValue=useCallback(async(label:string,value?:string)=>{if(!value)return;await navigator.clipboard.writeText(value);setCopied(label);toast.success(`${label} copied`);window.setTimeout(()=>setCopied(c=>c===label?undefined:c),1500)},[])
  const exportArtifact=useCallback((artifact:FederatedArtifact)=>{const label=artifact.name??artifact.descriptor?.name??artifact.kind??'artifact';const blob=new Blob([`${JSON.stringify(artifact,null,2)}\n`],{type:'application/json'}),url=URL.createObjectURL(blob),anchor=document.createElement('a');anchor.href=url;anchor.download=`${label.toLowerCase().replace(/[^a-z0-9._-]+/g,'-')}.depot.json`;anchor.click();URL.revokeObjectURL(url);toast.success('Artifact metadata exported')},[])
  const visible = visibleArtifacts(state.window)
  const resultCount=state.total??state.window.rowCount

  return <>
    <AppHeader breadcrumbs={[{label:'Depot'},{label:'Discover'}]}/>
    <div className={`${AURORA_PAGE_SHELL} flex-1`}><div className={AURORA_PAGE_FRAME}>
      <ConsoleHero eyebrow="Depot · Bazaar" title="Discover" description="Browse the bounded artifact catalog reported by configured Depot providers." pulse={{color:state.error?'var(--aurora-warn)':'var(--aurora-success)',label:state.coverage??'ready'}} actions={<Badge variant="outline" className="h-8 gap-1.5 border-aurora-warn/45 px-3 text-aurora-warn"><Lock className="size-3"/>Read-only</Badge>}>
        <div className="space-y-3"><p className="px-1 text-[11px] font-semibold text-aurora-text-muted">{state.loading?'Loading catalog…':`${resultCount} artifact${resultCount===1?'':'s'} reported by Depot`}</p>
          <div className="flex flex-col gap-2 xl:flex-row"><label className="sr-only" htmlFor="depot-provider">Depot provider</label><select id="depot-provider" value={selectedProvider} onChange={event=>{lanes.current.invalidate('list');const params=new URLSearchParams(window.location.search);params.set('provider',event.target.value);params.delete('artifact');params.delete('artifactProvider');router.replace(`${pathname}?${params}`,{scroll:false})}} className="h-11 rounded-aurora-2 border border-aurora-border-subtle bg-aurora-control-surface px-3 text-sm text-aurora-text-primary"><option value="all">All providers</option>{providers.map(provider=><option key={provider.id} value={provider.id} disabled={!provider.enabled}>{provider.name} · {provider.id}{provider.enabled?'':' (disabled)'}</option>)}</select><div className="relative min-w-0 flex-1"><Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-accent-primary"/><Input aria-label="Search Depot artifacts" className="h-11 rounded-aurora-2 border-aurora-accent-primary/70 bg-aurora-control-surface pl-10 text-sm" value={query} onChange={e=>{lanes.current.invalidate('list');setQuery(e.target.value)}} placeholder="Search Depot artifacts"/></div>
            <div className="flex h-11 items-center rounded-aurora-2 border border-aurora-border-subtle bg-aurora-control-surface p-1">{([['cards',Grid2X2],['list',List]] as const).map(([mode,Icon])=><button key={mode} type="button" onClick={()=>setView(mode)} aria-label={`${mode} view`} aria-pressed={view===mode} className="rounded-aurora-1 p-2 text-aurora-text-muted aria-pressed:bg-aurora-selected-bg aria-pressed:text-aurora-accent-primary"><Icon className="size-4"/></button>)}</div>
          </div></div>
      </ConsoleHero>
      {state.error?<DashboardPanel title="Depot unavailable"><p role="alert" className="text-sm text-aurora-error">{state.error}. Labby-only routes remain available.</p></DashboardPanel>:null}
      <section aria-labelledby="artifact-results-title" className="space-y-3"><div className="flex items-end justify-between gap-3 px-0.5"><h2 id="artifact-results-title" className="text-base font-semibold text-aurora-text-primary">{activeQuery?`Results for “${activeQuery}”`:'Catalog results'}</h2><span className="text-[11px] font-semibold text-aurora-text-muted">{state.loading?'Searching…':`${state.window.rowCount} retained of ${resultCount}`}</span></div>
        {state.window.historyExpired?<p role="status" className="text-xs text-aurora-text-muted">Earlier results left the bounded local window. Refresh this search to revisit older history.</p>:null}
        {visible.leadingRows>0?<div aria-hidden="true" style={{height:Math.min(visible.leadingRows*8,320)}} />:null}
        {query.length>0&&query.length<3?<p className="text-sm text-aurora-text-muted">Enter at least 3 characters to search.</p>:<ArtifactResults artifacts={visible.items} loading={state.loading} view={view} selectedKey={selectedId&&selectedArtifactProvider?artifactKey(selectedArtifactProvider,selectedId):undefined} artifactHref={artifactHref}/>} 
        {state.cursor?<div ref={loadMoreRef} className="flex min-h-12 items-center justify-center" role="status" aria-live="polite"><Button variant="outline" onClick={()=>void load(activeQuery,state.cursor)} disabled={state.loading}>{state.loading?<Loader2 className="size-4 animate-spin"/>:null}{state.loading?'Loading more artifacts…':'Load more'}</Button></div>:null}
      </section>
    </div></div>
    <ArtifactInspection artifact={detail} loading={detailLoading} open={Boolean(selectedId&&selectedArtifactProvider)} closeHref={artifactHref()} copied={copied} onOpenChange={open=>{if(!open)router.push(artifactHref(),{scroll:false})}} onCopy={copyValue} onExport={exportArtifact}/>
  </>
}

function ArtifactResults({artifacts,loading,view,selectedKey,artifactHref}:{artifacts:FederatedArtifact[];loading:boolean;view:View;selectedKey?:string;artifactHref:(providerId?:string,id?:string)=>string}){
  if(loading&&!artifacts.length)return <div className="flex min-h-56 items-center justify-center rounded-aurora-2 border border-dashed border-aurora-border-subtle text-sm text-aurora-text-muted"><Loader2 className="mr-2 size-4 animate-spin"/>Searching Bazaar…</div>
  if(!artifacts.length)return <div className="flex min-h-56 items-center justify-center rounded-aurora-2 border border-dashed border-aurora-border-subtle text-sm text-aurora-text-muted">No artifacts match this search.</div>
  return <div className={view==='cards'?'grid gap-3 md:grid-cols-2 2xl:grid-cols-4':'space-y-2'}>{artifacts.map(artifact=><ArtifactCard key={artifactKey(artifact.providerId,artifact.artifactId)} artifact={artifact} compact={view==='list'} selected={selectedKey===artifactKey(artifact.providerId,artifact.artifactId)} href={artifactHref(artifact.providerId,artifact.artifactId)}/>)}</div>
}

function ArtifactCard({artifact,compact,selected,href}:{artifact:FederatedArtifact;compact:boolean;selected:boolean;href:string}){
  const label=artifact.title||artifact.name||artifact.descriptor?.title||artifact.descriptor?.name||artifact.id||'Untitled artifact',kind=artifact.kind??artifact.descriptor?.kind??'artifact',namespace=artifact.namespace??artifact.descriptor?.namespace??'Unknown namespace'
  return <a href={href} aria-current={selected?'page':undefined} className={`group block rounded-aurora-2 border bg-aurora-panel-medium shadow-sm transition-all hover:-translate-y-0.5 hover:border-aurora-accent-primary/55 aria-[current=page]:border-aurora-accent-primary ${compact?'p-3':'min-h-[210px] p-4'}`}><div className={compact?'grid items-center gap-3 md:grid-cols-[minmax(0,1fr)_9rem_12rem]':'flex h-full flex-col'}><div className="min-w-0"><div className="flex items-center justify-between gap-2"><span className="flex items-center gap-2 text-[10px] font-bold uppercase tracking-[.12em] text-aurora-accent-primary"><span className="grid size-7 place-items-center rounded-aurora-1 border border-current/30"><Layers3 className="size-3.5"/></span>{kind}</span><Badge variant="outline">{artifact.providerId}</Badge></div><h3 className="mt-3 truncate text-base font-semibold text-aurora-text-primary">{label}</h3><p className="mt-0.5 truncate text-[11px] text-aurora-text-muted">{namespace} · {artifact.providerId}</p>{!compact?<><p className="mt-3 line-clamp-2 min-h-10 text-xs leading-5 text-aurora-text-muted">{artifact.description??artifact.descriptor?.description??'No description supplied by Depot.'}</p><div className="mt-3 flex gap-1.5"><Badge variant="outline">#{kind}</Badge>{artifact.publication?.visibility?<Badge variant="outline">{artifact.publication.visibility}</Badge>:null}</div></>:null}</div>{compact?<p className="truncate text-xs text-aurora-text-muted">{kind} · {namespace}</p>:null}<div className={`${compact?'':'mt-auto border-t border-aurora-border-subtle pt-3'} flex items-center justify-between gap-3 text-[10px] text-aurora-text-muted`}><span>{artifact.artifactId}</span><span>{artifact.contentDigest||artifact.currentRevision?.contentDigest?'Digest available':'Digest unavailable'}</span></div></div></a>
}

function ArtifactInspection({artifact,loading,open,closeHref,copied,onOpenChange,onCopy,onExport}:{artifact:FederatedArtifact|null;loading:boolean;open:boolean;closeHref:string;copied?:string;onOpenChange:(open:boolean)=>void;onCopy:(label:string,value?:string)=>void;onExport:(artifact:FederatedArtifact)=>void}){
  const title=artifact?.title??artifact?.descriptor?.title??artifact?.name??artifact?.descriptor?.name??'Artifact inspection',kind=artifact?.kind??artifact?.descriptor?.kind??'artifact',namespace=artifact?.namespace??artifact?.descriptor?.namespace??'Unknown namespace'
  return <Sheet open={open} onOpenChange={onOpenChange}><SheetContent className="w-[min(35rem,94vw)] max-w-none gap-0 border-aurora-border-subtle bg-aurora-panel-strong p-0 sm:max-w-none"><a href={closeHref} aria-label="Close artifact inspection" className="absolute right-3.5 top-3.5 z-20 rounded-aurora-1 bg-aurora-panel-strong p-1.5 text-aurora-text-muted transition-colors hover:bg-aurora-surface-muted hover:text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2"><X className="size-4"/></a><SheetHeader className="border-b border-aurora-border-subtle px-5 py-4 pr-12"><div className="flex items-start gap-3"><span className="grid size-10 shrink-0 place-items-center rounded-aurora-2 border border-aurora-accent-primary/45 bg-aurora-selected-bg text-aurora-accent-primary"><Layers3 className="size-4"/></span><div className="min-w-0"><SheetTitle className="truncate text-xl text-aurora-text-primary">{title}</SheetTitle><SheetDescription className="mt-0.5 text-xs">{namespace} · {kind} · Depot artifact</SheetDescription></div></div></SheetHeader>
    {loading?<div className="flex flex-1 items-center justify-center text-sm text-aurora-text-muted"><Loader2 className="mr-2 size-4 animate-spin"/>Loading artifact…</div>:artifact?<div className="flex-1 space-y-4 overflow-y-auto p-5"><p className="text-sm leading-6 text-aurora-text-primary">{artifact.description??artifact.descriptor?.description??'No description supplied by Depot.'}</p><div className="flex flex-wrap gap-2"><Badge variant="outline">#{kind}</Badge>{artifact.publication?.state?<Badge variant="outline">{artifact.publication.state}</Badge>:null}{artifact.publication?.visibility?<Badge variant="outline">{artifact.publication.visibility}</Badge>:null}</div><dl className="grid grid-cols-2 gap-px overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-aurora-border-subtle">{([['Revision count',artifact.revisionCount?.toString()],['Distribution',artifact.publication?.distribution],['License review',artifact.license?.reviewState],['Redistribution',artifact.license?.redistribution]] as const).map(([label,value])=><div key={label} className="bg-aurora-panel-low p-3"><dt className="text-[9px] font-bold uppercase tracking-wider text-aurora-text-muted">{label}</dt><dd className="mt-1 text-sm font-semibold text-aurora-text-primary">{value||'Not supplied'}</dd></div>)}</dl>{([['Artifact ID',artifact.id],['Revision ID',artifact.currentRevisionId??artifact.currentRevision?.id],['Content digest',artifact.contentDigest??artifact.currentRevision?.contentDigest]] as const).map(([label,value])=>value?<div key={label} className="flex items-center gap-2 rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low p-3"><div className="min-w-0 flex-1"><p className="text-[9px] font-bold uppercase tracking-wider text-aurora-text-muted">{label}</p><code className="block truncate pt-1 text-xs text-aurora-text-primary" title={value}>{value}</code></div><Button variant="ghost" size="icon-sm" aria-label={`Copy ${label}`} onClick={()=>void onCopy(label,value)}>{copied===label?<Check className="size-4 text-aurora-success"/>:<Copy className="size-4"/>}</Button></div>:null)}<div className="rounded-aurora-1 border border-aurora-warn/35 bg-aurora-warn/5 p-3 text-xs leading-5 text-aurora-text-muted"><Lock className="mr-1.5 inline size-3.5 text-aurora-warn"/>Install, fork, and import actions are unavailable on this read-only Depot connection.</div><div className="flex flex-wrap gap-2 border-t border-aurora-border-subtle pt-4"><Button variant="outline" size="sm" onClick={()=>onExport(artifact)}><Download className="size-4"/>Export metadata</Button><Button variant="outline" size="sm" onClick={()=>void onCopy('Artifact link',window.location.href)}><Link2 className="size-4"/>Copy link</Button></div></div>:<div className="flex flex-1 items-center justify-center text-sm text-aurora-text-muted">Artifact details are unavailable.</div>}</SheetContent></Sheet>
}
