'use client'

import { Check, Grid2X2, List, ListCollapse, Rows2, SlidersHorizontal } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import type { DiscoverySort } from './discover-model'

export type DiscoveryDensity = 'compact' | 'comfortable'
export type DiscoveryLayout = 'cards' | 'list'

export function DiscoverViewOptions({ sort, setSort, layout, setLayout, density, setDensity }: {
  sort: DiscoverySort
  setSort: (value: DiscoverySort) => void
  layout: DiscoveryLayout
  setLayout: (value: DiscoveryLayout) => void
  density: DiscoveryDensity
  setDensity: (value: DiscoveryDensity) => void
}) {
  return <Popover>
    <PopoverTrigger asChild><Button data-visible-label variant="outline" size="icon-sm" className="size-[26px] rounded-[8px]" style={{ width: 26, minWidth: 26, maxWidth: 26, height: 26 }} aria-label="Sort, density and layout" title="Sort, density and layout"><SlidersHorizontal aria-hidden="true" className="size-3" /></Button></PopoverTrigger>
    <PopoverContent align="end" sideOffset={6} aria-label="Sort, density and layout" className="box-content flex w-[220px] max-w-[calc(100vw-2rem)] flex-col gap-[3px] rounded-[12px] border-[color-mix(in_srgb,var(--aurora-border-strong)_75%,var(--aurora-page-bg))] bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] p-1.5 text-aurora-text-primary shadow-[var(--aurora-shadow-strong),inset_0_1px_0_rgba(255,255,255,0.05)]">
      <div role="group" aria-label="Sort retained results" className="contents">
        <span className="px-2 pb-px pt-[5px] text-[9px] font-bold uppercase leading-[11px] tracking-[0.13em] text-[#97b6c9]">Sort</span>
        {([['relevance','Relevance'],['newest','Recently Updated'],['installs','Most Installed'],['stars','Most Starred']] as const).map(([value,label]) => <Button key={value} variant={sort===value?'secondary':'ghost'} data-visible-label className="h-7 w-full justify-between gap-2 rounded-[8px] border-0 px-[9px] text-xs font-semibold" aria-pressed={sort===value} onClick={()=>setSort(value)}><span>{label}</span>{sort===value?<Check aria-hidden className="size-3 text-aurora-accent-strong"/>:null}</Button>)}
      </div>
      <span aria-hidden className="mx-1.5 my-[3px] h-px shrink-0 bg-[color-mix(in_srgb,var(--aurora-border-default)_50%,transparent)]" />
      <div role="group" aria-label="Density" className="flex items-center gap-2 px-2 pb-1 pt-0.5"><span className="flex-1 text-[9px] font-bold uppercase tracking-[0.13em] text-[#97b6c9]">Density</span><div className="flex gap-[3px] rounded-[9px] border border-[color-mix(in_srgb,var(--aurora-border-default)_55%,var(--aurora-page-bg))] bg-[var(--gw0-0_40)] p-[3px]">{([['comfortable','Comfortable',Rows2],['compact','Compact',ListCollapse]] as const).map(([value,label,Icon])=><Button data-visible-label key={value} size="icon-sm" className="rounded-[6px]" style={{ width:28,minWidth:28,maxWidth:28,height:22,minHeight:22,maxHeight:22 }} variant={density===value?'secondary':'ghost'} aria-label={label} title={label} aria-pressed={density===value} onClick={()=>setDensity(value)}><Icon aria-hidden className="size-3.5"/></Button>)}</div></div>
      <div role="group" aria-label="Layout" className="flex items-center gap-2 px-2 pb-1"><span className="flex-1 text-[9px] font-bold uppercase tracking-[0.13em] text-[#97b6c9]">Layout</span><div className="flex gap-[3px] rounded-[9px] border border-[color-mix(in_srgb,var(--aurora-border-default)_55%,var(--aurora-page-bg))] bg-[var(--gw0-0_40)] p-[3px]">{([['cards','Grid view',Grid2X2],['list','List view',List]] as const).map(([value,label,Icon])=><Button data-visible-label key={value} size="icon-sm" className="rounded-[6px]" style={{ width:28,minWidth:28,maxWidth:28,height:22,minHeight:22,maxHeight:22 }} variant={layout===value?'secondary':'ghost'} aria-label={label} title={label} aria-pressed={layout===value} onClick={()=>setLayout(value)}><Icon aria-hidden className="size-3.5"/></Button>)}</div></div>
    </PopoverContent>
  </Popover>
}
