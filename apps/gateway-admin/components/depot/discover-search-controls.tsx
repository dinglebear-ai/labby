'use client'

import { Globe, Github, Network, Search, Server, SlidersHorizontal, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import type { ArtifactSourceOrigin, DepotProviderOption, FederatedArtifact } from '@/lib/api/depot-client'
import { DISCOVERY_KINDS } from '@/lib/depot/provider-model'
import { ClaudeMark } from './discover-format-mark'
import { BRAND_PATHS } from './brand-paths'
import { discoverKindPresentation } from './discover-kind-presentation'

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

function ProviderMark({ origin }: { origin?: ArtifactSourceOrigin }) {
  if (origin === 'claude') return <ClaudeMark className="size-3.5 text-aurora-accent-pink" />
  if (origin === 'gemini') return <svg aria-hidden="true" viewBox="0 0 24 24" className="size-3.5 fill-current text-aurora-accent-primary">{BRAND_PATHS.googlegemini.map((path, index) => <path key={index} d={path} />)}</svg>
  const Icon = origin === 'github' ? Github : origin === 'web-crawl' ? Globe : origin === 'mcp-registry' ? Server : Network
  return <Icon aria-hidden="true" className="size-3.5" strokeWidth={1.7} />
}

export function DiscoverSearchControls({ query, onQuery, providers, artifacts, kind, selectedProvider, onFilter }: {
  query: string
  onQuery: (value: string) => void
  providers: DepotProviderOption[]
  artifacts: readonly FederatedArtifact[]
  kind: string
  selectedProvider: string
  onFilter: (field: 'kind' | 'provider', value: string) => void
}) {
  const origins = discoveryProviderOrigins(artifacts)
  const filterCount = Number(kind !== 'all') + Number(selectedProvider !== 'all')
  return <div className="px-6 py-3.5 sm:pl-[82px]">
    <div data-discover-search="1" className="flex h-[42px] min-w-0 items-center gap-[9px] rounded-xl border border-aurora-border-strong bg-aurora-control-surface px-[13px] transition-shadow focus-within:border-aurora-accent-primary/70 focus-within:shadow-[0_0_0_1px_color-mix(in_srgb,var(--aurora-accent-primary)_45%,transparent),0_0_18px_3px_color-mix(in_srgb,var(--aurora-accent-primary)_22%,transparent)]">
      <Search aria-hidden="true" className="size-4 shrink-0 text-aurora-accent-strong" strokeWidth={1.7} />
      <Input name="artifact-search" aria-label="Search Depot artifacts" value={query} onChange={event => onQuery(event.target.value)} placeholder="Search artifacts across your sources" className="h-10 min-w-0 flex-1 border-0 bg-transparent px-0 text-sm font-medium shadow-none focus-visible:ring-0" />
      {providers.length > 0 ? <><span aria-hidden className="h-[18px] w-px shrink-0 bg-aurora-border-default/70 max-sm:hidden" /><div role="group" aria-label="Depot sources" className="aurora-scrollbar flex max-w-[42%] shrink-0 items-center gap-[3px] overflow-x-auto max-sm:hidden">
        {providers.map(provider => <Button data-visible-label key={provider.id} variant="ghost" size="icon-sm" disabled={!provider.enabled} aria-label={`Filter to ${provider.name}`} title={`${provider.name} — filter results`} aria-pressed={selectedProvider === provider.id} onClick={() => onFilter('provider', selectedProvider === provider.id ? 'all' : provider.id)} className="size-[26px] shrink-0 rounded-[7px] border p-0" style={{ color: 'var(--aurora-accent-strong)', borderColor: selectedProvider === provider.id ? 'color-mix(in srgb, var(--aurora-accent-primary) 55%, transparent)' : 'transparent', background: selectedProvider === provider.id ? 'color-mix(in srgb, var(--aurora-accent-primary) 10%, transparent)' : 'transparent' }}><ProviderMark origin={origins.get(provider.id)} /></Button>)}
      </div></> : null}
      <span aria-hidden className="h-[18px] w-px shrink-0 bg-aurora-border-default/70" />
      <Popover>
        <PopoverTrigger asChild><Button data-visible-label variant="ghost" size="icon-sm" className="relative size-[30px] shrink-0 rounded-[9px] border border-aurora-border-default/55" aria-label="Kind and source filters"><SlidersHorizontal aria-hidden className="size-[15px]" />{filterCount > 0 ? <span className="absolute -right-1 -top-1 grid min-w-[15px] place-items-center rounded-full border border-aurora-panel-strong bg-aurora-accent-primary px-1 text-[9px] font-bold text-aurora-page-bg">{filterCount}</span> : null}</Button></PopoverTrigger>
        <PopoverContent align="end" aria-label="Kind and source filters" className="aurora-scrollbar max-h-[min(32rem,70svh)] w-[min(580px,calc(100vw-2rem))] space-y-4 overflow-y-auto rounded-xl border-aurora-border-strong bg-aurora-panel-strong p-4 text-aurora-text-primary">
          <fieldset className="space-y-2"><legend className="text-[9.5px] font-bold uppercase tracking-[0.13em] text-aurora-text-muted">Kind</legend><div className="flex flex-wrap gap-1.5">{['all', ...DISCOVERY_KINDS].map(value => { const presentation = discoverKindPresentation(value); const Icon = presentation.icon; return <Button data-visible-label key={value} size="sm" variant={kind === value ? 'secondary' : 'ghost'} className="h-[27px] rounded-lg px-2.5 text-[11px] font-[650]" aria-pressed={kind === value} onClick={() => onFilter('kind', value)}><Icon aria-hidden className="size-3" style={{ color: presentation.color }} />{value === 'all' ? 'All kinds' : value}</Button> })}</div></fieldset>
          <fieldset className="space-y-2"><legend className="text-[9.5px] font-bold uppercase tracking-[0.13em] text-aurora-text-muted">Source</legend><div className="flex flex-wrap gap-1.5">{[{ id: 'all', name: 'All sources', enabled: true }, ...providers].map(provider => <Button data-visible-label key={provider.id} size="sm" className="h-[26px] rounded-lg px-2.5 text-[10.5px] font-[650]" variant={selectedProvider === provider.id ? 'secondary' : 'ghost'} disabled={!provider.enabled} aria-pressed={selectedProvider === provider.id} onClick={() => onFilter('provider', provider.id)}><ProviderMark origin={origins.get(provider.id)} />{provider.name}</Button>)}</div></fieldset>
          <p className="text-[10px] text-aurora-text-muted">Kinds filter the full catalog before pagination.</p>
        </PopoverContent>
      </Popover>
    </div>
    {filterCount > 0 ? <div className="flex flex-wrap items-center gap-[7px] pt-2"><span className="text-[9.5px] font-bold uppercase tracking-[0.13em] text-aurora-text-muted">Applied</span>{([{ field: 'kind', label: kind }, { field: 'provider', label: providers.find(provider => provider.id === selectedProvider)?.name ?? selectedProvider }] as const).filter(filter => filter.field === 'kind' ? kind !== 'all' : selectedProvider !== 'all').map(filter => <span key={filter.field} className="inline-flex h-[25px] max-w-[320px] items-center gap-[7px] rounded-full border border-aurora-accent-pink/35 bg-aurora-accent-pink/10 pl-2.5 pr-1.5 text-[11px] font-[650]"><span className="truncate">{filter.label}</span><Button data-visible-label variant="ghost" size="icon-sm" aria-label={`Remove ${filter.field} filter`} onClick={() => onFilter(filter.field, 'all')} className="size-4 rounded-full p-0"><X className="size-2.5" /></Button></span>)}</div> : null}
  </div>
}
