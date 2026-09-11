'use client'

import { Check, Grid2X2, List, ListCollapse, Rows2, Rows3, SlidersHorizontal } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import type { DiscoverySort } from './discover-model'

export type DiscoveryDensity = 'compact' | 'default' | 'comfortable'
export type DiscoveryLayout = 'cards' | 'list'

export function DiscoverViewOptions({ sort, setSort, layout, setLayout, density, setDensity, ranked = false }: {
  ranked?: boolean
  sort: DiscoverySort
  setSort: (value: DiscoverySort) => void
  layout: DiscoveryLayout
  setLayout: (value: DiscoveryLayout) => void
  density: DiscoveryDensity
  setDensity: (value: DiscoveryDensity) => void
}) {
  return <Popover>
    <PopoverTrigger asChild><Button variant="outline" size="icon-sm" className="size-[26px] rounded-[8px]" aria-label="Sort, density and layout"><SlidersHorizontal aria-hidden="true" className="size-3" /></Button></PopoverTrigger>
    <PopoverContent align="end" aria-label="Sort, density and layout" className="w-[220px] max-w-[calc(100vw-2rem)] space-y-3 rounded-[12px] border-aurora-border-strong bg-aurora-panel-strong p-2 text-aurora-text-primary">
      {ranked ? <p className="px-2 text-xs text-aurora-text-muted">Newest first-seen, ranked across sources.</p> : <div role="group" aria-label="Sort retained results" className="space-y-0.5">
        {([['catalog', 'Catalog order'], ['newest', 'Recently updated'], ['name', 'Name']] as const).map(([value, label]) => <Button
          key={value} variant={sort === value ? 'secondary' : 'ghost'} data-visible-label
          className="h-7 w-full justify-between rounded-[8px] px-[9px] text-xs font-semibold"
          aria-pressed={sort === value} onClick={() => setSort(value)}
        ><span>{label}</span>{sort === value ? <Check aria-hidden="true" className="size-3 text-aurora-accent-strong" /> : null}</Button>)}
      </div>}
      <div className="mx-1.5 border-t border-aurora-border-default" />
      <div role="group" aria-label="Density" className="flex items-center gap-2 px-2">
        <span className="flex-1 text-[9px] font-bold uppercase tracking-[0.13em] text-aurora-text-muted">Density</span>
        <div className="flex gap-[3px] rounded-[9px] border border-aurora-border-default bg-aurora-control-surface p-[3px]">
          {([['compact', 'Compact', ListCollapse], ['default', 'Default', Rows3], ['comfortable', 'Comfortable', Rows2]] as const).map(([value, label, Icon]) => <Button
            key={value} size="icon-sm" className="h-[22px] w-7 rounded-[6px]"
            variant={density === value ? 'secondary' : 'ghost'} aria-label={label} title={label}
            aria-pressed={density === value} onClick={() => setDensity(value)}
          ><Icon aria-hidden="true" className="size-3.5" /></Button>)}
        </div>
      </div>
      <div role="group" aria-label="Layout" className="flex items-center gap-2 px-2">
        <span className="flex-1 text-[9px] font-bold uppercase tracking-[0.13em] text-aurora-text-muted">Layout</span>
        <div className="flex gap-[3px] rounded-[9px] border border-aurora-border-default bg-aurora-control-surface p-[3px]">
          {([['cards', 'Cards', Grid2X2], ['list', 'List', List]] as const).map(([value, label, Icon]) => <Button
            key={value} size="icon-sm" className="h-[22px] w-7 rounded-[6px]"
            variant={layout === value ? 'secondary' : 'ghost'} aria-label={label} title={label}
            aria-pressed={layout === value} onClick={() => setLayout(value)}
          ><Icon aria-hidden="true" className="size-3.5" /></Button>)}
        </div>
      </div>
      {!ranked ? <p className="px-2 text-[10px] leading-snug text-aurora-text-muted">Sorting applies to loaded results, not the entire source catalog.</p> : null}
    </PopoverContent>
  </Popover>
}
