'use client'

import { Clock3, Download, GitFork, LayoutGrid, ShieldCheck } from 'lucide-react'
import { Button } from '@/components/ui/button'
import type { FederatedArtifact } from '@/lib/api/depot-client'
import type { DiscoverySort } from './discover-model'

const tabs = [
  { sort: 'catalog', label: 'Catalog', icon: LayoutGrid },
  { sort: 'newest', label: 'New', icon: Clock3 },
  { sort: 'installs', label: 'Popular', icon: Download },
  { sort: 'forks', label: 'Forks', icon: GitFork },
  { sort: 'verified', label: 'Verified', icon: ShieldCheck },
] as const

export function DiscoverResultTabs({ artifacts, sort, setSort }: { artifacts: readonly FederatedArtifact[]; sort: DiscoverySort; setSort: (sort: DiscoverySort) => void }) {
  return <div role="group" aria-label="Artifact result order" className="aurora-scrollbar flex min-w-0 items-center gap-0.5 overflow-x-auto">
    {tabs.map(tab => {
      const available = tab.sort === 'catalog' || artifacts.some(artifact => tab.sort === 'newest' ? Boolean(artifact.currentRevision?.authoredAt) : tab.sort === 'verified' ? artifact.publisherVerified !== undefined : artifact.metrics?.[tab.sort] !== undefined)
      const title = !available ? 'Connected sources have not supplied this metadata.' : tab.sort === 'catalog' ? 'Order supplied by the catalog.' : tab.sort === 'newest' ? 'Revision dates in loaded results.' : tab.sort === 'verified' ? 'Publisher verification reported in loaded results.' : `${tab.sort === 'installs' ? 'Installs' : 'Forks'} reported in loaded results.`
      return <Button data-visible-label key={tab.sort} variant="ghost" size="sm" disabled={!available} aria-pressed={sort === tab.sort} title={title} onClick={() => setSort(tab.sort)} className="h-[34px] shrink-0 gap-1.5 rounded-none border-0 border-b-2 px-3 text-xs font-[650]" style={{ borderBottomColor: sort === tab.sort ? 'var(--aurora-accent-primary)' : 'transparent', color: sort === tab.sort ? 'var(--aurora-text-primary)' : 'var(--aurora-text-muted)' }}><tab.icon aria-hidden className="size-[13px]" strokeWidth={1.7} />{tab.label}</Button>
    })}
  </div>
}
