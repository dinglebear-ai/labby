'use client'

import { Plus } from 'lucide-react'
import { ConsoleHero } from '@/components/console/console-hero'
import { Button } from '@/components/ui/button'

export function DevContainersHero({ rows, onCreate }: { rows: string[][]; onCreate: () => void }) {
  const building = rows.filter(row => row[0] === 'Building')
  const published = rows.filter(row => row[0] === 'Ready')
  const pulls = published.reduce((total, row) => total + (Number(row[4]?.match(/^(\d+) pulls$/)?.[1]) || 0), 0)
  return <ConsoleHero variant="discover" eyebrow="Workspace · Incus" title="Dev Containers"
    description="System containers your team pulls onto their own machine. You define distro, toolchains, agents and loadouts — resource limits and mounts stay with whoever runs it."
    pulse={{ color: 'var(--aurora-accent-strong)', label: `${building.length} ${building.length === 1 ? 'image' : 'images'} building` }}
    actions={<Button variant="outline" data-visible-label="1" onClick={onCreate} className="h-9 gap-[7px] rounded-[10px] border-[color-mix(in_srgb,var(--aurora-accent-primary)_55%,var(--aurora-border-strong))] bg-[color-mix(in_srgb,var(--aurora-accent-primary)_9%,var(--aurora-panel-strong))] px-4 text-[13px] font-[650] text-aurora-accent-strong"><Plus className="size-3.5"/>New Container</Button>}
    stats={[
      { label: 'Images', value: published.length, suffix: 'published' },
      { label: 'Pulls', value: pulls, suffix: 'recorded' },
      { label: 'Members', value: '—', suffix: 'unavailable' },
      { label: 'Building', value: building.length, suffix: building.map(row => row[1]).join(', ') || 'none', tone: 'var(--aurora-accent-strong)' },
    ]}
  />
}
