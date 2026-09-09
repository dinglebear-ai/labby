'use client'

import { Plus } from 'lucide-react'
import { ConsoleHero } from '@/components/console/console-hero'
import { Button } from '@/components/ui/button'
import { WorkspaceStats } from './workspace-stats'

export function AgentsHero({ rows, onCreate }: { rows: string[][]; onCreate: () => void }) {
  const count = (status: string) => rows.filter(row => row[0] === status).length
  return <ConsoleHero eyebrow="Team · Agents" title="Agents" pulse={{ color: 'var(--aurora-success)', label: `${count('Running')} running` }} actions={<Button variant="outline" data-visible-label="1" onClick={onCreate} className="h-9 gap-[7px] rounded-[10px] border-[color-mix(in_srgb,var(--aurora-accent-primary)_55%,var(--aurora-border-strong))] bg-[color-mix(in_srgb,var(--aurora-accent-primary)_9%,var(--aurora-panel-strong))] px-4 text-[13px] font-[650] text-[#bfe7fb]"><Plus className="size-3.5"/>New Session</Button>} footer={<WorkspaceStats label="Agent statistics" stats={[
    { label: 'Running', value: count('Running'), suffix: 'sessions', color: 'var(--aurora-success)' },
    { label: 'Completed', value: count('Completed'), suffix: 'sessions', color: 'var(--aurora-text-primary)' },
    { label: 'Failed', value: count('Failed'), suffix: 'sessions', color: 'var(--aurora-error)' },
    { label: 'Median', value: '—', suffix: 'unavailable', color: 'var(--aurora-accent-strong)' },
  ]} />} />
}
