'use client'

import { Grid2X2, List, SlidersHorizontal } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import type { DiscoverySort } from './discover-model'

export type DiscoveryDensity = 'compact' | 'default' | 'comfortable'
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
    <PopoverTrigger asChild><Button variant="outline" size="icon-sm" aria-label="Sort, density and layout"><SlidersHorizontal aria-hidden="true" className="size-4" /></Button></PopoverTrigger>
    <PopoverContent align="end" aria-label="Sort, density and layout" className="w-80 max-w-[calc(100vw-2rem)] space-y-4 rounded-aurora-2 border-aurora-border-strong bg-aurora-panel-strong text-aurora-text-primary">
      <label className="block space-y-2 text-sm">Sort retained results
        <select value={sort} onChange={event => setSort(event.target.value as DiscoverySort)} className="block w-full rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-2 text-aurora-text-primary">
          <option value="catalog">Catalog order</option><option value="newest">Latest revision</option><option value="name">Name</option>
        </select>
      </label>
      <fieldset className="space-y-2"><legend className="text-sm">Layout</legend><div className="flex gap-2">
        {([['cards', Grid2X2], ['list', List]] as const).map(([value, Icon]) => <Button key={value} variant={layout === value ? 'secondary' : 'ghost'} aria-label={value === 'cards' ? 'Cards' : 'List'} aria-pressed={layout === value} onClick={() => setLayout(value)}><Icon aria-hidden="true" className="size-4" />{value}</Button>)}
      </div></fieldset>
      <fieldset className="space-y-2"><legend className="text-sm">Density</legend><div className="flex flex-wrap gap-2">
        {(['compact', 'default', 'comfortable'] as const).map(value => <Button key={value} size="sm" variant={density === value ? 'secondary' : 'ghost'} aria-pressed={density === value} onClick={() => setDensity(value)}>{value === 'default' ? 'Default' : value === 'compact' ? 'Compact' : 'Comfortable'}</Button>)}
      </div></fieldset>
      <p className="text-xs text-aurora-text-muted">Sorting applies to loaded results, not the entire source catalog.</p>
    </PopoverContent>
  </Popover>
}
