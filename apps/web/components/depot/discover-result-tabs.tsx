'use client'

import type { ComponentType } from 'react'
import { Clock3, Download, Flame, GitFork, Layers3, ShieldCheck } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { DISCOVERY_SHELVES, type DiscoveryShelf } from './discover-model'

const ICONS = {
  trending: Flame, new: Clock3, popular: Download, bundled: Layers3, forks: GitFork, curated: ShieldCheck,
} satisfies Record<DiscoveryShelf, ComponentType<{ className?: string; strokeWidth?: number }>>

export function DiscoverResultTabs({ shelf, setShelf }: { shelf: DiscoveryShelf; setShelf: (shelf: DiscoveryShelf) => void }) {
  return <div role="group" aria-label="Artifact shelves" className="aurora-scrollbar flex min-w-0 items-center gap-0.5 overflow-x-auto">
    {DISCOVERY_SHELVES.map(item => {
      const Icon = ICONS[item.id]
      const active = shelf === item.id
      return <Button data-visible-label key={item.id} variant="ghost" size="sm" aria-pressed={active} title={item.hint} onClick={() => setShelf(item.id)} className="-mb-px h-[34px] shrink-0 gap-1.5 rounded-none border-0 border-b-2 px-3 text-[12.5px] font-bold" style={{ paddingInline: 12, borderBottomColor: active ? 'var(--aurora-accent-primary)' : 'transparent', color: active ? 'var(--aurora-text-primary)' : 'var(--aurora-text-muted)' }}><Icon aria-hidden className="size-[13px]" strokeWidth={1.7} />{item.label}</Button>
    })}
  </div>
}
