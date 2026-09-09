'use client'

import { BadgeCheck, Clock3, Download, Flame, GitFork, Layers3 } from 'lucide-react'
import { Button } from '@/components/ui/button'

const feeds = [
  { id: 'trending', label: 'Trending', icon: Flame, hint: 'Velocity of installs and forks' },
  { id: 'new', label: 'New', icon: Clock3, hint: 'First seen in the last seven days' },
  { id: 'popular', label: 'Popular', icon: Download, hint: 'All-time installs across every target' },
  { id: 'bundled', label: 'Bundled', icon: Layers3, hint: 'Artifacts that ship together in loadouts' },
  { id: 'forks', label: 'Hot Forks', icon: GitFork, hint: 'Forks moving faster than upstream' },
  { id: 'curated', label: 'Curated', icon: BadgeCheck, hint: 'Verified publishers, reviewed by hand' },
] as const

/** Preserve the reference navigation without claiming unsupported server rankings. */
export function DiscoverFeedTabs({ selected, onSelect }: { selected?: 'new'; onSelect: (feed?: 'new') => void }) {
  return <div role="group" aria-label="Discovery feed" className="aurora-scrollbar flex min-w-0 items-end gap-0.5 overflow-x-auto">
    {feeds.map(({ id, label, icon: Icon, hint }) => {
      const available = id === 'new'
      const active = selected === id
      return <Button data-visible-label key={id} variant="ghost" size="sm" aria-pressed={active}
        aria-disabled={!available || undefined}
        title={available ? `${hint}${active ? ' — click again to return to the catalog' : ''}` : `${hint} — ranking is not supplied by the connected catalog`}
        onClick={() => { if (available) onSelect(active ? undefined : 'new') }}
        className="mb-[-1px] h-[34px] shrink-0 gap-1.5 rounded-none border-x-0 border-t-0 border-b-2 bg-transparent px-3 text-[12.5px] font-bold aria-disabled:cursor-not-allowed"
        style={{ borderBottomColor: active ? 'var(--aurora-accent-primary)' : 'transparent', color: active ? 'var(--aurora-accent-strong)' : 'var(--aurora-text-muted)' }}>
        <Icon aria-hidden="true" className="size-3.5" />{label}
      </Button>
    })}
  </div>
}
