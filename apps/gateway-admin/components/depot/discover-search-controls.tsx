'use client'

import { Globe, Github, Network, Search, Server, SlidersHorizontal, X, Users } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import type { ArtifactSourceOrigin, DepotProviderOption, FederatedArtifact } from '@/lib/api/depot-client'
import { DISCOVERY_KINDS } from '@/lib/depot/provider-model'
import { ClaudeMark } from './discover-format-mark'
import { BRAND_PATHS } from './brand-paths'
import { discoverKindPresentation } from './discover-kind-presentation'

const MOCK_KINDS = ['skill','agent','command','hook','prompt','mcp','acp','plugin','extension','loadout','snippet'] as const

/** A provider's brand is known only when its returned artifacts agree on an origin. */
export function discoveryProviderOrigins(artifacts: readonly FederatedArtifact[]): Map<string, ArtifactSourceOrigin> {
  const origins = new Map<string, Set<ArtifactSourceOrigin>>()
  for (const artifact of artifacts) {
    if (!artifact.sourceOrigin) continue
    const observed = origins.get(artifact.providerId) ?? new Set<ArtifactSourceOrigin>()
    observed.add(artifact.sourceOrigin)
    origins.set(artifact.providerId, observed)
  }
  return new Map([...origins].flatMap(([id, values]) => values.size === 1 ? [[id, [...values][0]] as const] : []))
}

export function DiscoverProviderMark({ origin }: { origin?: ArtifactSourceOrigin }) {
  if (origin === 'claude') return <ClaudeMark className="size-3.5 text-aurora-accent-pink" />
  if (origin === 'gemini') return <svg aria-hidden="true" viewBox="0 0 24 24" className="size-3.5 fill-current text-aurora-accent-primary">{BRAND_PATHS.googlegemini.map((path, index) => <path key={index} d={path} />)}</svg>
  const Icon = origin === 'github' ? Github : origin === 'web-crawl' ? Globe : origin === 'mcp-registry' ? Server : Network
  return <Icon aria-hidden="true" className="size-3.5" strokeWidth={1.7} />
}

export type DiscoveryVisibility = 'all' | 'public' | 'team'

type SearchProps = {
  query: string
  onQuery: (value: string) => void
  providers: DepotProviderOption[]
  artifacts: readonly FederatedArtifact[]
  kind: string
  selectedProvider: string
  onFilter: (field: 'kind' | 'provider', value: string) => void
  visibility?: DiscoveryVisibility
  onVisibility?: (value: DiscoveryVisibility) => void
  filtersOpen: boolean
  onFiltersOpenChange: (open: boolean) => void
  totalCount: number
  onClearAll: () => void
  showVisibility?: boolean
}

export function DiscoverSearchControls({ query, onQuery, providers, artifacts, kind, selectedProvider, onFilter, visibility = 'all', onVisibility, filtersOpen, onFiltersOpenChange, totalCount, onClearAll, showVisibility = true }: SearchProps) {
  const origins = discoveryProviderOrigins(artifacts)
  const filterCount = Number(kind !== 'all') + Number(selectedProvider !== 'all')
  const chipsOn = Boolean(query.trim()) || filterCount > 0 || (showVisibility && visibility !== 'all')
  return <>
  <div className="px-6 py-3.5 sm:pl-[82px]">
    <div data-discover-search="1" className="flex h-[44px] min-w-0 items-center gap-[9px] rounded-xl border border-aurora-border-strong bg-aurora-control-surface px-[13px] transition-shadow focus-within:border-aurora-accent-primary/70 focus-within:shadow-[0_0_0_1px_color-mix(in_srgb,var(--aurora-accent-primary)_45%,transparent),0_0_18px_3px_color-mix(in_srgb,var(--aurora-accent-primary)_22%,transparent)]">
      <Search aria-hidden="true" className="size-4 shrink-0 text-aurora-accent-strong" strokeWidth={1.7} />
      <Input name="artifact-search" aria-label="Search artifacts" value={query} onChange={event => onQuery(event.target.value)} placeholder={`Search ${totalCount} artifacts — semantic search`} className="h-[19px] min-w-0 flex-1 border-0 bg-transparent px-0 py-px text-sm font-medium shadow-none focus-visible:ring-0" />
      {providers.length > 0 ? <><span aria-hidden className="hidden h-[18px] w-px shrink-0 bg-aurora-border-default/70 sm:block" /><div role="group" aria-label="Labby sources" className="aurora-scrollbar hidden max-w-[42%] shrink-0 items-center gap-[3px] overflow-x-auto sm:flex">
        {providers.map(provider => <Button data-visible-label key={provider.id} variant="ghost" size="icon-sm" disabled={!provider.enabled} aria-label={`Filter to ${provider.name.replace(/\bDepot\b/gi, 'Labby')}`} title={`${provider.name.replace(/\bDepot\b/gi, 'Labby')} — filter results`} aria-pressed={selectedProvider === provider.id} onClick={() => { onFilter('provider', selectedProvider === provider.id ? 'all' : provider.id); onFiltersOpenChange(true) }} className="size-[26px] shrink-0 rounded-[7px] border p-0" style={{ color: 'var(--aurora-accent-strong)', borderColor: selectedProvider === provider.id ? 'color-mix(in srgb, var(--aurora-accent-primary) 55%, transparent)' : 'color-mix(in srgb, var(--aurora-border-default) 50%, var(--aurora-page-bg))', background: selectedProvider === provider.id ? 'color-mix(in srgb, var(--aurora-accent-primary) 10%, transparent)' : 'var(--gw0-0_40)' }}><DiscoverProviderMark origin={origins.get(provider.id)} /></Button>)}
      </div></> : null}
      <span aria-hidden className="h-[18px] w-px shrink-0 bg-aurora-border-default/70" />
      <Button data-visible-label variant="ghost" size="icon-sm" className="relative size-[30px] shrink-0 rounded-[9px] border" style={{ color: filtersOpen || filterCount ? 'var(--aurora-accent-strong)' : 'var(--aurora-text-muted)', borderColor: filtersOpen || filterCount ? 'color-mix(in srgb, var(--aurora-accent-primary) 40%, transparent)' : 'color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))', background: filtersOpen || filterCount ? 'color-mix(in srgb, var(--aurora-accent-primary) 9%, transparent)' : 'var(--gw0-0_40)' }} aria-label="Kind and source filters" aria-expanded={filtersOpen} onClick={() => onFiltersOpenChange(!filtersOpen)}><SlidersHorizontal aria-hidden className="size-[15px]" />{filterCount > 0 ? <span className="absolute -right-1 -top-1 grid min-w-[15px] place-items-center rounded-full border border-aurora-panel-strong bg-aurora-accent-primary px-1 text-[9px] font-bold text-aurora-page-bg">{filterCount}</span> : null}</Button>
    </div>
  </div>
  {chipsOn ? <div className="flex flex-wrap items-center gap-[7px] px-6 pb-[5px] pt-[9px] sm:pl-[82px]"><span className="text-[9.5px] font-bold uppercase leading-[11px] tracking-[0.13em] text-aurora-text-muted">Applied</span>{query.trim() ? <FilterChip label={`Search: “${query.trim()}”`} onClear={()=>onQuery('')} /> : null}{kind !== 'all' ? <FilterChip label={`Kind: ${kind}`} onClear={()=>onFilter('kind','all')} /> : null}{selectedProvider !== 'all' ? <FilterChip label={`Source: ${providers.find(provider=>provider.id===selectedProvider)?.name ?? selectedProvider}`} onClear={()=>onFilter('provider','all')} /> : null}{showVisibility && visibility !== 'all' ? <FilterChip label={`Access: ${visibility}`} onClear={()=>onVisibility?.('all')} /> : null}<Button data-visible-label variant="ghost" size="sm" onClick={onClearAll} className="h-[25px] rounded-full border border-transparent px-[11px] text-[11px] font-[650] text-aurora-text-muted">Clear All</Button></div> : null}
  </>
}

function FilterChip({label,onClear}:{label:string;onClear:()=>void}) {
  return <span className="inline-flex h-[25px] max-w-[320px] items-center gap-[7px] rounded-full border border-aurora-accent-pink/35 bg-aurora-accent-pink/10 pl-2.5 pr-1.5 text-[11px] font-[650]"><span className="truncate">{label}</span><Button data-visible-label variant="ghost" size="icon-sm" aria-label={`Remove ${label} filter`} onClick={onClear} className="size-4 rounded-full p-0"><X className="size-2.5" /></Button></span>
}

export function DiscoverFilterPanel({ open, providers, artifacts, kind, selectedProvider, onFilter, visibility='all', onVisibility, mockKinds=false, showVisibility=true }: { open:boolean; providers:DepotProviderOption[]; artifacts:readonly FederatedArtifact[]; kind:string; selectedProvider:string; onFilter:(field:'kind'|'provider',value:string)=>void; visibility?:DiscoveryVisibility; onVisibility?:(value:DiscoveryVisibility)=>void; mockKinds?:boolean; showVisibility?:boolean }) {
  if (!open) return null
  const origins=discoveryProviderOrigins(artifacts)
  const kinds=mockKinds ? [...MOCK_KINDS] : [...DISCOVERY_KINDS]
  return <section aria-label="Kind and source filters" className="flex flex-col gap-[11px] rounded-aurora-2 border border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))] bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] px-[15px] pb-[14px] pt-[13px] shadow-[var(--aurora-shadow-medium),inset_0_1px_0_rgba(255,255,255,0.04)]">
    <FilterSection label="Kind" onAny={()=>onFilter('kind','all')}><Button data-visible-label size="sm" variant={kind==='all'?'secondary':'ghost'} className="h-[27px] rounded-lg px-2.5 text-[11px]" aria-pressed={kind==='all'} onClick={()=>onFilter('kind','all')}>All <span className="text-[9.5px] opacity-70">{artifacts.length}</span></Button>{kinds.map(value=>{const presentation=discoverKindPresentation(value);const Icon=presentation.icon;const count=artifacts.filter(a=>(a.kind??a.descriptor?.kind)===value).length;return <Button data-visible-label key={value} size="sm" variant={kind===value?'secondary':'ghost'} className="h-[27px] rounded-lg px-2.5 text-[11px] font-[650]" aria-pressed={kind===value} onClick={()=>onFilter('kind',kind===value?'all':value)}><Icon aria-hidden className="size-3" style={{color:presentation.color}}/>{value}<span className="text-[9.5px] opacity-70">{count}</span></Button>})}</FilterSection>
    <FilterSection label="Source" onAny={()=>onFilter('provider','all')}><Button data-visible-label size="sm" variant={selectedProvider==='all'?'secondary':'ghost'} className="h-[26px] rounded-lg px-2.5 text-[10.5px]" aria-pressed={selectedProvider==='all'} onClick={()=>onFilter('provider','all')}><Network className="size-3"/>All Sources</Button>{providers.map(provider=><Button data-visible-label key={provider.id} size="sm" variant={selectedProvider===provider.id?'secondary':'ghost'} disabled={!provider.enabled} className="h-[26px] rounded-lg px-2.5 text-[10.5px] font-[650]" aria-pressed={selectedProvider===provider.id} title={`Filter to ${provider.name}`} onClick={()=>onFilter('provider',selectedProvider===provider.id?'all':provider.id)}><DiscoverProviderMark origin={origins.get(provider.id)}/>{provider.name}</Button>)}</FilterSection>
    {showVisibility && onVisibility ? <FilterSection label="Result access" onAny={()=>onVisibility('all')}><p className="basis-full text-[10px] text-aurora-text-muted">Loaded-result visibility reported by each source.</p>{([['all','All',Network],['public','Public',Globe],['team','Team',Users]] as const).map(([value,label,Icon])=><Button data-visible-label key={value} size="sm" variant={visibility===value?'secondary':'ghost'} className="h-[26px] rounded-lg px-2.5 text-[10.5px]" aria-pressed={visibility===value} onClick={()=>onVisibility(value)}><Icon className="size-3"/>{label}</Button>)}</FilterSection> : null}
  </section>
}

function FilterSection({label,onAny,children}:{label:string;onAny:()=>void;children:React.ReactNode}) {
  return <fieldset className="flex min-w-0 flex-col gap-2"><legend className="sr-only">{label}</legend><div className="flex items-center gap-2"><span className="shrink-0 text-[9.5px] font-bold uppercase leading-[11px] tracking-[0.13em] text-[#97b6c9]">{label}</span><span className="h-px flex-1 bg-aurora-border-default/40"/><Button data-visible-label type="button" variant="ghost" size="sm" onClick={onAny} className="h-[18px] rounded-[5px] px-[7px] text-[9.5px] font-bold uppercase tracking-[0.08em] text-[#9bbacd]">Any</Button></div><div className="flex flex-wrap items-center gap-1.5">{children}</div></fieldset>
}
