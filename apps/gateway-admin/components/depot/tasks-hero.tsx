'use client'

import { Plus } from 'lucide-react'
import { ConsoleHero } from '@/components/console/console-hero'
import { Button } from '@/components/ui/button'
import { WorkspaceStats } from './workspace-stats'

export function TasksHero({ rows, onCreate }: { rows: string[][]; onCreate: () => void }) {
  const stats = [
    { label: 'Scheduled', value: rows.length, suffix: 'tasks', color: 'var(--aurora-text-primary)' },
    { label: 'Armed', value: rows.filter(row => row[0] === 'Armed').length, suffix: 'armed', color: 'var(--aurora-success)' },
    { label: 'Next Run', value: '02:00', suffix: 'Scope Audit', color: 'var(--aurora-accent-strong)' },
    { label: 'Failures', value: rows.filter(row => row[6] === 'failed').length, suffix: 'last run', color: 'var(--aurora-error)' },
  ]
  return <ConsoleHero eyebrow="Team · Schedules" title="Tasks" description={<span className="block max-w-[540px] text-pretty">Recurring agent runs. Each task carries its own loadout, container and repository, and reports back into Activity when it finishes.</span>} actions={<Button variant="outline" data-visible-label="1" onClick={onCreate} className="h-9 gap-[7px] rounded-[10px] border-[color-mix(in_srgb,var(--aurora-accent-primary)_55%,var(--aurora-border-strong))] bg-[color-mix(in_srgb,var(--aurora-accent-primary)_9%,var(--aurora-panel-strong))] px-4 text-[13px] font-[650] text-[#bfe7fb]"><Plus className="size-3.5"/>New Task</Button>} footer={<WorkspaceStats label="Task statistics" stats={stats} />} />
}
