'use client'

import { useCallback, useEffect, useState } from 'react'
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
import { depotCall, depotStatus, type DepotArtifact, type DepotStatus } from '@/lib/api/depot-client'

type LoadState = { loading: boolean; error?: string; status?: DepotStatus; artifacts: DepotArtifact[]; cursor?: string; total?: number }
type View = 'cards' | 'list'

export function DepotPageContent() {
  const router = useRouter(), pathname = usePathname(), searchParams = useSearchParams()
  const selectedId = searchParams.get('artifact')?.trim(), initialQuery = searchParams.get('q')?.trim() ?? ''
  const [query,setQuery] = useState(initialQuery), [activeQuery,setActiveQuery] = useState(initialQuery)
  const [state,setState] = useState<LoadState>({ loading:true, artifacts:[] })
  const [detail,setDetail] = useState<DepotArtifact|null>(null), [detailLoading,setDetailLoading] = useState(false)
  const [copied,setCopied] = useState<string>(), [view,setView] = useState<View>('cards')

  const load = useCallback(async (searchQuery:string,cursor?:string,signal?:AbortSignal) => {
    setState(c=>({...c,loading:true,error:undefined,artifacts:cursor?c.artifacts:[],cursor:cursor?c.cursor:undefined,total:cursor?c.total:undefined}))
    try {
      const [status,listing] = await Promise.all([depotStatus(signal),depotCall<{result?:{artifacts?:DepotArtifact[];nextCursor?:string;total?:number}}>('depot.artifacts.list',{limit:50,...(searchQuery?{query:searchQuery}:{}),...(cursor?{cursor}:{})},signal)])
      setState(c=>({loading:false,status,artifacts:cursor?[...c.artifacts,...(listing.result?.artifacts??[])]:listing.result?.artifacts??[],cursor:listing.result?.nextCursor,total:listing.result?.total}))
    } catch(error) { if(!signal?.aborted)setState(c=>({...c,loading:false,error:error instanceof Error?error.message:String(error)})) }
  },[])

  useEffect(()=>{ const controller=new AbortController(); const timer=window.setTimeout(()=>{ const next=query.trim(); setActiveQuery(next); const params=new URLSearchParams(window.location.search); if((params.get('q')?.trim()??'')!==next){if(next)params.set('q',next);else params.delete('q');params.delete('artifact');router.replace(`${pathname}${params.size?`?${params}`:''}`,{scroll:false})} void load(next,undefined,controller.signal)},query?300:0); return()=>{window.clearTimeout(timer);controller.abort()} },[load,pathname,query,router])
  useEffect(()=>{if(!selectedId){setDetail(null);return}const controller=new AbortController();setDetailLoading(true);void depotCall<{result?:{artifact?:DepotArtifact}}>('depot.artifacts.get',{artifactId:selectedId},controller.signal).then(r=>setDetail(r.result?.artifact??null)).catch(e=>{if(!controller.signal.aborted)toast.error(e instanceof Error?e.message:String(e))}).finally(()=>{if(!controller.signal.aborted)setDetailLoading(false)});return()=>controller.abort()},[selectedId])

  const artifactHref=useCallback((id?:string)=>{const params=new URLSearchParams();if(activeQuery)params.set('q',activeQuery);if(id)params.set('artifact',id);return `${pathname}${params.size?`?${params}`:''}`},[activeQuery,pathname])
  const copyValue=useCallback(async(label:string,value?:string)=>{if(!value)return;await navigator.clipboard.writeText(value);setCopied(label);toast.success(`${label} copied`);window.setTimeout(()=>setCopied(c=>c===label?undefined:c),1500)},[])
  const exportArtifact=useCallback((artifact:DepotArtifact)=>{const label=artifact.name??artifact.descriptor?.name??artifact.kind??'artifact';const blob=new Blob([`${JSON.stringify(artifact,null,2)}\n`],{type:'application/json'}),url=URL.createObjectURL(blob),anchor=document.createElement('a');anchor.href=url;anchor.download=`${label.toLowerCase().replace(/[^a-z0-9._-]+/g,'-')}.depot.json`;anchor.click();URL.revokeObjectURL(url);toast.success('Artifact metadata exported')},[])
  const resultCount=state.total??state.artifacts.length

  return <>
    <AppHeader breadcrumbs={[{label:'Depot'},{label:'Discovery'}]}/>
    <div className={`${AURORA_PAGE_SHELL} flex-1`}><div className={AURORA_PAGE_FRAME}>
      <ConsoleHero eyebrow="Depot · Bazaar" title="Discovery" description="Browse the bounded artifact catalog reported by the connected Depot control plane." pulse={{color:state.status?.enabled?'var(--aurora-success)':'var(--aurora-warn)',label:state.status?.enabled?'connected':'disabled'}} actions={<Badge variant="outline" className="h-8 gap-1.5 border-aurora-warn/45 px-3 text-aurora-warn"><Lock className="size-3"/>Read-only</Badge>}>
        <div className="space-y-3"><p className="px-1 text-[11px] font-semibold text-aurora-text-muted">{state.loading?'Loading catalog…':`${resultCount} artifact${resultCount===1?'':'s'} reported by Depot`}</p>
          <div className="flex flex-col gap-2 xl:flex-row"><div className="relative min-w-0 flex-1"><Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-accent-primary"/><Input aria-label="Search Depot artifacts" className="h-11 rounded-aurora-2 border-aurora-accent-primary/70 bg-aurora-control-surface pl-10 text-sm" value={query} onChange={e=>setQuery(e.target.value)} placeholder="Search Depot artifacts"/></div>
            <div className="flex h-11 items-center rounded-aurora-2 border border-aurora-border-subtle bg-aurora-control-surface p-1">{([['cards',Grid2X2],['list',List]] as const).map(([mode,Icon])=><button key={mode} type="button" onClick={()=>setView(mode)} aria-label={`${mode} view`} aria-pressed={view===mode} className="rounded-aurora-1 p-2 text-aurora-text-muted aria-pressed:bg-aurora-selected-bg aria-pressed:text-aurora-accent-primary"><Icon className="size-4"/></button>)}</div>
          </div></div>
      </ConsoleHero>
      {state.error?<DashboardPanel title="Depot unavailable"><p role="alert" className="text-sm text-aurora-error">{state.error}. Labby-only routes remain available.</p></DashboardPanel>:null}
      <section aria-labelledby="artifact-results-title" className="space-y-3"><div className="flex items-end justify-between gap-3 px-0.5"><h2 id="artifact-results-title" className="text-base font-semibold text-aurora-text-primary">{activeQuery?`Results for “${activeQuery}”`:'Catalog results'}</h2><span className="text-[11px] font-semibold text-aurora-text-muted">{state.loading?'Searching…':`${state.artifacts.length} of ${resultCount} shown`}</span></div>
        <ArtifactResults artifacts={state.artifacts} loading={state.loading} view={view} selectedId={selectedId} artifactHref={artifactHref}/>
        {state.cursor?<Button variant="outline" onClick={()=>void load(activeQuery,state.cursor)} disabled={state.loading}>{state.loading?<Loader2 className="size-4 animate-spin"/>:null}Load more</Button>:null}
      </section>
    </div></div>
    <ArtifactInspection artifact={detail} loading={detailLoading} open={Boolean(selectedId)} closeHref={artifactHref()} copied={copied} onOpenChange={open=>{if(!open)router.push(artifactHref(),{scroll:false})}} onCopy={copyValue} onExport={exportArtifact}/>
  </>
}

function ArtifactResults({artifacts,loading,view,selectedId,artifactHref}:{artifacts:DepotArtifact[];loading:boolean;view:View;selectedId?:string;artifactHref:(id?:string)=>string}){
  if(loading&&!artifacts.length)return <div className="flex min-h-56 items-center justify-center rounded-aurora-2 border border-dashed border-aurora-border-subtle text-sm text-aurora-text-muted"><Loader2 className="mr-2 size-4 animate-spin"/>Searching Bazaar…</div>
  if(!artifacts.length)return <div className="flex min-h-56 items-center justify-center rounded-aurora-2 border border-dashed border-aurora-border-subtle text-sm text-aurora-text-muted">No artifacts match this search.</div>
  return <div className={view==='cards'?'grid gap-3 md:grid-cols-2 2xl:grid-cols-4':'space-y-2'}>{artifacts.map((artifact,index)=><ArtifactCard key={artifact.id??index} artifact={artifact} compact={view==='list'} selected={selectedId===artifact.id} href={artifactHref(artifact.id)}/>)}</div>
}

function ArtifactCard({artifact,compact,selected,href}:{artifact:DepotArtifact;compact:boolean;selected:boolean;href:string}){
  const label=artifact.title||artifact.name||artifact.descriptor?.title||artifact.descriptor?.name||artifact.id||'Untitled artifact',kind=artifact.kind??artifact.descriptor?.kind??'artifact',namespace=artifact.namespace??artifact.descriptor?.namespace??'Unknown namespace'
  return <a href={href} aria-current={selected?'page':undefined} className={`group block rounded-aurora-2 border bg-aurora-panel-medium shadow-sm transition-all hover:-translate-y-0.5 hover:border-aurora-accent-primary/55 aria-[current=page]:border-aurora-accent-primary ${compact?'p-3':'min-h-[210px] p-4'}`}><div className={compact?'grid items-center gap-3 md:grid-cols-[minmax(0,1fr)_9rem_12rem]':'flex h-full flex-col'}><div className="min-w-0"><div className="flex items-center justify-between gap-2"><span className="flex items-center gap-2 text-[10px] font-bold uppercase tracking-[.12em] text-aurora-accent-primary"><span className="grid size-7 place-items-center rounded-aurora-1 border border-current/30"><Layers3 className="size-3.5"/></span>{kind}</span>{artifact.publication?.state?<Badge variant="outline" className="max-w-28 truncate text-[8px] uppercase tracking-wider">{artifact.publication.state}</Badge>:null}</div><h3 className="mt-3 truncate text-base font-semibold text-aurora-text-primary">{label}</h3><p className="mt-0.5 truncate text-[11px] text-aurora-text-muted">{namespace}</p>{!compact?<><p className="mt-3 line-clamp-2 min-h-10 text-xs leading-5 text-aurora-text-muted">{artifact.description??artifact.descriptor?.description??'No description supplied by Depot.'}</p><div className="mt-3 flex gap-1.5"><Badge variant="outline">#{kind}</Badge>{artifact.publication?.visibility?<Badge variant="outline">{artifact.publication.visibility}</Badge>:null}</div></>:null}</div>{compact?<p className="truncate text-xs text-aurora-text-muted">{kind} · {namespace}</p>:null}<div className={`${compact?'':'mt-auto border-t border-aurora-border-subtle pt-3'} flex items-center justify-between gap-3 text-[10px] text-aurora-text-muted`}><span>{artifact.revisionCount===undefined?'Revision count unavailable':`${artifact.revisionCount} revision${artifact.revisionCount===1?'':'s'}`}</span><span>{artifact.contentDigest||artifact.currentRevision?.contentDigest?'Digest available':'Digest unavailable'}</span></div></div></a>
}

function ArtifactInspection({artifact,loading,open,closeHref,copied,onOpenChange,onCopy,onExport}:{artifact:DepotArtifact|null;loading:boolean;open:boolean;closeHref:string;copied?:string;onOpenChange:(open:boolean)=>void;onCopy:(label:string,value?:string)=>void;onExport:(artifact:DepotArtifact)=>void}){
  const title=artifact?.title??artifact?.descriptor?.title??artifact?.name??artifact?.descriptor?.name??'Artifact inspection',kind=artifact?.kind??artifact?.descriptor?.kind??'artifact',namespace=artifact?.namespace??artifact?.descriptor?.namespace??'Unknown namespace'
  return <Sheet open={open} onOpenChange={onOpenChange}><SheetContent className="w-[min(35rem,94vw)] max-w-none gap-0 border-aurora-border-subtle bg-aurora-panel-strong p-0 sm:max-w-none"><a href={closeHref} aria-label="Close artifact inspection" className="absolute right-3.5 top-3.5 z-20 rounded-aurora-1 bg-aurora-panel-strong p-1.5 text-aurora-text-muted transition-colors hover:bg-aurora-surface-muted hover:text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2"><X className="size-4"/></a><SheetHeader className="border-b border-aurora-border-subtle px-5 py-4 pr-12"><div className="flex items-start gap-3"><span className="grid size-10 shrink-0 place-items-center rounded-aurora-2 border border-aurora-accent-primary/45 bg-aurora-selected-bg text-aurora-accent-primary"><Layers3 className="size-4"/></span><div className="min-w-0"><SheetTitle className="truncate text-xl text-aurora-text-primary">{title}</SheetTitle><SheetDescription className="mt-0.5 text-xs">{namespace} · {kind} · Depot artifact</SheetDescription></div></div></SheetHeader>
    {loading?<div className="flex flex-1 items-center justify-center text-sm text-aurora-text-muted"><Loader2 className="mr-2 size-4 animate-spin"/>Loading artifact…</div>:artifact?<div className="flex-1 space-y-4 overflow-y-auto p-5"><p className="text-sm leading-6 text-aurora-text-primary">{artifact.description??artifact.descriptor?.description??'No description supplied by Depot.'}</p><div className="flex flex-wrap gap-2"><Badge variant="outline">#{kind}</Badge>{artifact.publication?.state?<Badge variant="outline">{artifact.publication.state}</Badge>:null}{artifact.publication?.visibility?<Badge variant="outline">{artifact.publication.visibility}</Badge>:null}</div><dl className="grid grid-cols-2 gap-px overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-aurora-border-subtle">{([['Revision count',artifact.revisionCount?.toString()],['Distribution',artifact.publication?.distribution],['License review',artifact.license?.reviewState],['Redistribution',artifact.license?.redistribution]] as const).map(([label,value])=><div key={label} className="bg-aurora-panel-low p-3"><dt className="text-[9px] font-bold uppercase tracking-wider text-aurora-text-muted">{label}</dt><dd className="mt-1 text-sm font-semibold text-aurora-text-primary">{value||'Not supplied'}</dd></div>)}</dl>{([['Artifact ID',artifact.id],['Revision ID',artifact.currentRevisionId??artifact.currentRevision?.id],['Content digest',artifact.contentDigest??artifact.currentRevision?.contentDigest]] as const).map(([label,value])=>value?<div key={label} className="flex items-center gap-2 rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low p-3"><div className="min-w-0 flex-1"><p className="text-[9px] font-bold uppercase tracking-wider text-aurora-text-muted">{label}</p><code className="block truncate pt-1 text-xs text-aurora-text-primary" title={value}>{value}</code></div><Button variant="ghost" size="icon-sm" aria-label={`Copy ${label}`} onClick={()=>void onCopy(label,value)}>{copied===label?<Check className="size-4 text-aurora-success"/>:<Copy className="size-4"/>}</Button></div>:null)}<div className="rounded-aurora-1 border border-aurora-warn/35 bg-aurora-warn/5 p-3 text-xs leading-5 text-aurora-text-muted"><Lock className="mr-1.5 inline size-3.5 text-aurora-warn"/>Install, fork, and import actions are unavailable on this read-only Depot connection.</div><div className="flex flex-wrap gap-2 border-t border-aurora-border-subtle pt-4"><Button variant="outline" size="sm" onClick={()=>onExport(artifact)}><Download className="size-4"/>Export metadata</Button><Button variant="outline" size="sm" onClick={()=>void onCopy('Artifact link',window.location.href)}><Link2 className="size-4"/>Copy link</Button></div></div>:<div className="flex flex-1 items-center justify-center text-sm text-aurora-text-muted">Artifact details are unavailable.</div>}</SheetContent></Sheet>
}
